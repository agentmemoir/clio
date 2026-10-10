// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the library-level raw-ingest entry point:
//! accepting turn/tool text and running the full parallel ingest pipeline.
//!
//! ## Owns
//! - `RawIngestOptions` (bundled caller inputs) and the item-shell assembly
//! - `ingest_raw_with_extractor` (explicit extractor) and `ingest_raw`
//!   (config-resolved extractor, fail-closed when extraction is unavailable)
//!
//! ## Does not own
//! - Chunk → extract → span verify → gated store orchestration (`ingest`)
//! - Extractor provider selection (`extract_factory`) or admission scoring
//!
//! ## Boundary
//!
//! Changes to the raw-ingest entry surface should normally require changes here.
//! Changes to verification, admission, or persistence should not.
//!
//! Keep this module focused on the raw-ingest entry surface.

use crate::chunk::Turn;
use crate::extract::{Extractor, HttpTransport};
use crate::extract_factory::build_extractor_from;
use crate::ingest::{IngestOutcome, ParallelIngestArgs, WriteIngestConfig, parallel_ingest};
use crate::maintenance::{LeafIndex, SharedMaintenanceHook};
use crate::schema::{FieldSpec, default_event_fields};
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{AmError, EpistemicKind, ErrorCode, MemoryItem, OpContext};
use serde_json::Value;

/// Bundled inputs for the raw-ingest entry points.
pub struct RawIngestOptions<'a> {
    /// Repository used for the gated store step (writes item + index job).
    pub store: &'a dyn clio_store::Store,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Session turns / tool text to ingest.
    pub turns: &'a [Turn],
    /// Event timestamp applied to every admitted item.
    pub timestamp: &'a str,
    /// Prefix for the shell item id; `parallel_ingest` appends `-0-{seq}`.
    pub item_id_prefix: &'a str,
    /// Admission candidate discriminator.
    pub candidate: Candidate,
    /// Neighbor/prior signal source for five-factor admission.
    ///
    /// Hosts with a store-backed source pass it so novelty and recency score
    /// against existing rows; [`EmptySignals`] is the neutral choice for hosts
    /// that intentionally do not score against stored memory.
    pub signal_source: &'a dyn SignalSource,
    /// Snapshot schema template for the extractor.
    pub schema_template: Value,
    /// Optional bounded source context (descriptive, untrusted).
    ///
    /// When supplied it is validated with `clio_types::validate_context`
    /// before any extraction runs, and sealed into every shell item so
    /// admitted items retain it after store and leaf publication.
    pub context: Option<String>,
    /// Ingest knobs.
    pub config: WriteIngestConfig,
    /// Leaf index for immediate get-by-id.
    pub leaves: &'a LeafIndex,
    /// Maintenance handoff (dispatched off the write path).
    pub maintenance: SharedMaintenanceHook,
}

/// Neutral signal source: no neighbors and no prior.
///
/// Use only when a host intentionally does not score against stored memory
/// (tests, fixtures). Gated write hosts SHOULD pass a store-backed source so the
/// duplicate gate can fire against existing rows.
#[derive(Debug, Default)]
pub struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }

    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

fn item_shell(options: &RawIngestOptions<'_>) -> MemoryItem {
    let base_id = format!("{}-0", options.item_id_prefix);
    let (category, episodic_type) = match options.candidate {
        Candidate::Semantic(c) => (Some(c), None),
        Candidate::Episodic(t) => (None, Some(t)),
    };
    MemoryItem {
        id: base_id.clone(),
        bank_id: options.ctx.bank.as_str().to_owned(),
        subject_id: base_id,
        kind: options.candidate.kind(),
        category,
        episodic_type,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: options.timestamp.to_owned(),
        valid_until: None,
        tx_from: options.timestamp.to_owned(),
        tx_until: None,
        created_at: options.timestamp.to_owned(),
        updated_at: options.timestamp.to_owned(),
        context: options.context.clone(),
    }
}

/// Run the full pipeline with an explicit extractor.
///
/// Chunk → extract → span verify (retry once, then refuse) → gated store →
/// index enqueue. The write closure calls `Store::create_memory_item`, which
/// transactionally enqueues the durable index job; admission and span verify
/// are never bypassed. A supplied `context` is validated before extraction,
/// so an empty or oversized value fails closed with nothing extracted or
/// written.
///
/// # Errors
/// Invalid source `context` (empty or oversized); bank isolation failures;
/// gated store structural errors.
pub fn ingest_raw_with_extractor(
    options: &RawIngestOptions<'_>,
    extractor: &(dyn Extractor + Sync),
) -> Result<IngestOutcome, AmError> {
    if let Some(context) = &options.context {
        clio_types::validate_context(context)?;
    }
    let fields: Vec<FieldSpec> = default_event_fields();
    let verify = VerifyConfig::default();
    let env = RankingEnv::system_defaults();
    let admit_sink = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let mut write = |item: &MemoryItem| {
        options
            .store
            .create_memory_item(item, options.ctx.actor.as_str())
    };
    parallel_ingest(
        &ParallelIngestArgs {
            extractor,
            turns: options.turns,
            schema_template: options.schema_template.clone(),
            fields: &fields,
            verify: &verify,
            item_base: item_shell(options),
            candidate: options.candidate,
            signal_source: options.signal_source,
            ctx: options.ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit_sink,
            verify_sink: &verify_sink,
            timestamp: options.timestamp,
            config: options.config.clone(),
            leaves: options.leaves,
            maintenance: options.maintenance.clone(),
        },
        &mut write,
    )
}

/// Run the full pipeline with the extractor resolved from effective config.
///
/// Fail-closed when no local extraction endpoint is configured: with no
/// `extract.url` there is no extractor to run, so this returns
/// `NotImplemented` rather than fabricating a candidate. Hosted extraction
/// stays opt-in through `extract.provider = openai` plus a credential.
///
/// # Errors
/// Extraction unavailable (no configured endpoint); extractor build failures.
pub fn ingest_raw(
    options: &RawIngestOptions<'_>,
    effective: &Value,
) -> Result<IngestOutcome, AmError> {
    let url = effective
        .pointer("/extract/url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if url.is_empty() {
        return Err(AmError::new(
            ErrorCode::NotImplemented,
            "extraction unavailable: no extractor configured (set extract.url to a local extraction sidecar, or extract.provider=openai with a credential); no candidate was fabricated",
        ));
    }
    let bearer = clio_config::config::bearer_for_effective(effective, "extract");
    let extractor = build_extractor_from(effective, bearer, HttpTransport::default())?;
    ingest_raw_with_extractor(options, &*extractor)
}

#[cfg(test)]
#[path = "raw_ingest_tests.rs"]
mod raw_ingest_tests;
