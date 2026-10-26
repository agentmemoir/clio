// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for parallel ingest: chunk → extract → consolidate → gated leaf.
//!
//! ## Owns
//! - Write-path orchestration for multi-chunk sessions
//! - Partial-success vs all-or-nothing policy
//! - Leaf publish timing + non-blocking maintenance handoff
//!
//! ## Does not own
//! - MemTree ancestor refresh or embedding rebuilds
//!
//! ## Boundary
//!
//! Changes to parallel ingest flow should normally require changes here.
//! Changes to extraction, span verify, or admission scoring should not require changes here.
//!
//! Keep this module focused on parallel ingest orchestration.

use crate::chunk::{DEFAULT_MAX_TURNS_PER_CHUNK, Turn, partition_session};
use crate::consolidate::{CanonicalUnit, VerifiedCandidate, consolidate};
use crate::extract::Extractor;
use crate::maintenance::{LeafIndex, MaintenanceSignal, SharedMaintenanceHook, dispatch_async};
use crate::parallel::{ChunkWorkerResult, ParallelExtractAggregate, extract_chunks_parallel};
use crate::pipeline::NonAuthoritativeRetention;
use crate::schema::FieldSpec;
use crate::store_path::{VerifiedStoreRequest, gated_store_verified};
use crate::telemetry::VerifySink;
use crate::verify::VerifyConfig;
use clio_admission::decision::{Candidate, Decision};
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, EventSink};
use clio_types::{AmError, ErrorCode, MemoryItem, OpContext};
use serde_json::Value;
use std::time::Instant;

/// Default bounded parallelism for chunk extract workers.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Knobs for [`parallel_ingest`].
#[derive(Debug, Clone, PartialEq)]
pub struct WriteIngestConfig {
    /// Max concurrent chunk workers (`P`).
    pub concurrency: usize,
    /// Max turns per extraction chunk.
    pub max_turns_per_chunk: usize,
    /// When true, any chunk failure aborts before admit.
    pub all_or_nothing: bool,
    /// Embedding/LLM merge (MUST remain false as sole authority; default off).
    pub embedding_merge: bool,
    /// Profile label for timing metrics (`A` / `B` / `C` / custom).
    pub profile_label: String,
}

impl Default for WriteIngestConfig {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            max_turns_per_chunk: DEFAULT_MAX_TURNS_PER_CHUNK,
            all_or_nothing: false,
            embedding_merge: false,
            profile_label: "reference".into(),
        }
    }
}

impl WriteIngestConfig {
    /// Config under a named deployment profile.
    ///
    /// `concurrency` follows the profile's vCPU budget from the hardware profiles
    /// (A `api-inference` = 1, B `local-retrieval` = 2, C `local-rft` = 4);
    /// unknown labels keep [`DEFAULT_CONCURRENCY`]. The timing harness runs the
    /// same fixture extract under each profile switch.
    #[must_use]
    pub fn for_profile(label: impl Into<String>) -> Self {
        let label = label.into();
        let concurrency = match label.as_str() {
            "A" | "api-inference" => 1,
            "B" | "local-retrieval" => 2,
            "C" | "local-rft" => 4,
            _ => DEFAULT_CONCURRENCY,
        };
        Self {
            concurrency,
            profile_label: label,
            ..Self::default()
        }
    }
}

/// Timing metrics for the leaf-first ingest path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IngestTiming {
    /// Ingest elapsed time through the final admitted leaf being queryable (ms).
    pub time_to_queryable_ms: u128,
    /// Wall-clock for parallel extract phase only (ms).
    pub extract_phase_ms: u128,
    /// Structural maintenance duration (ms); zero while async work is pending.
    pub structural_maintenance_ms: u128,
}

/// One admitted leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmittedLeaf {
    /// Item id.
    pub item_id: String,
    /// Admission decision.
    pub decision: Decision,
    /// `true` once published to the leaf index.
    pub leaf_queryable: bool,
    /// Chunk ids consolidated into this leaf.
    pub chunk_ids: Vec<String>,
}

/// Full parallel ingest outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestOutcome {
    /// Admitted leaves (may be empty).
    pub admitted: Vec<AdmittedLeaf>,
    /// Per-chunk worker results.
    pub chunk_results: Vec<ChunkWorkerResult>,
    /// Non-authoritative retentions from refused chunks.
    pub retained: Vec<NonAuthoritativeRetention>,
    /// How many near-duplicate merges occurred.
    pub consolidate_merges: u32,
    /// How many discrete-key contradiction splits occurred.
    pub contradiction_splits: u32,
    /// True when at least one leaf was published queryable.
    pub leaf_queryable: bool,
    /// Maintenance signals emitted (also delivered to the hook).
    pub maintenance_signals: Vec<MaintenanceSignal>,
    /// Timing breakdown.
    pub timing: IngestTiming,
    /// Profile label used for this run.
    pub profile_label: String,
}

/// Bundled inputs for [`parallel_ingest`].
pub struct ParallelIngestArgs<'a> {
    /// Sync extractor (shared across workers).
    pub extractor: &'a (dyn Extractor + Sync),
    /// Session turns.
    pub turns: &'a [Turn],
    /// Snapshot schema template.
    pub schema_template: Value,
    /// Extractive field specs.
    pub fields: &'a [FieldSpec],
    /// Span-verify knobs.
    pub verify: &'a VerifyConfig,
    /// Item shell template (id rewritten per unit).
    pub item_base: MemoryItem,
    /// Admission candidate kind.
    pub candidate: Candidate,
    /// Novelty/recency signals.
    pub signal_source: &'a dyn SignalSource,
    /// Bank/actor context.
    pub ctx: &'a OpContext,
    /// Admission policy.
    pub policy: AdmissionPolicy<'a>,
    /// Admission sink.
    pub admit_sink: &'a dyn EventSink,
    /// Verify sink (used on post-merge re-verify / gated store).
    pub verify_sink: &'a dyn VerifySink,
    /// Timestamp label.
    pub timestamp: &'a str,
    /// Ingest knobs.
    pub config: WriteIngestConfig,
    /// Leaf index for immediate get-by-id.
    pub leaves: &'a LeafIndex,
    /// Maintenance handoff (dispatched off the write path; must not gate readability).
    pub maintenance: SharedMaintenanceHook,
}

fn bank_guard(ctx: &OpContext, item: &MemoryItem) -> Result<(), AmError> {
    if ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if item.bank_id != ctx.bank.as_str() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "cross-bank ingest rejected: item.bank_id must match context bank",
        ));
    }
    Ok(())
}

fn empty_outcome(
    profile: &str,
    extract_phase_ms: u128,
    chunk_results: Vec<ChunkWorkerResult>,
    retained: Vec<NonAuthoritativeRetention>,
) -> IngestOutcome {
    IngestOutcome {
        admitted: vec![],
        chunk_results,
        retained,
        consolidate_merges: 0,
        contradiction_splits: 0,
        leaf_queryable: false,
        maintenance_signals: vec![],
        timing: IngestTiming {
            time_to_queryable_ms: 0,
            extract_phase_ms,
            structural_maintenance_ms: 0,
        },
        profile_label: profile.to_owned(),
    }
}

fn to_verified(agg: &ParallelExtractAggregate) -> Vec<VerifiedCandidate> {
    agg.verified()
        .into_iter()
        .map(|(chunk_id, source_text, candidate)| VerifiedCandidate {
            chunk_id,
            source_text,
            candidate,
            confidence: 1.0,
        })
        .collect()
}

/// Admit each canonical unit through the gated store and publish queryable leaves.
///
/// Snapshot immutability: consolidation keeps the first contributing candidate's
/// snapshot as canonical (see [`crate::consolidate`]); merge never rewrites it, only
/// unions provenance/source text. Post-merge span verification is therefore not a
/// separate branch — [`crate::store_path::gated_store_verified`] re-runs
/// `verify_snapshot` against the merged source before commit, so every admitted
/// snapshot is still grounded in the union of its source chunks.
fn admit_canonical_units(
    args: &ParallelIngestArgs<'_>,
    units: &[CanonicalUnit],
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
    ingest_started: Instant,
) -> Result<(Vec<AdmittedLeaf>, Vec<MaintenanceSignal>, u128), AmError> {
    let mut admitted = Vec::new();
    let mut maintenance_signals = Vec::new();
    let mut time_to_queryable_ms = 0;
    for (seq, unit) in units.iter().enumerate() {
        let mut item = args.item_base.clone();
        item.id = format!("{}-{seq}", args.item_base.id);
        item.snapshot.clone_from(&unit.snapshot);
        item.gist.clone_from(&unit.gist);
        item.source_ref = Some(unit.source_refs.join(","));
        item.confidence = Some(unit.confidence);
        let source_owned = unit.source_text.clone();
        let mut req = VerifiedStoreRequest {
            item,
            ctx: args.ctx,
            source_text: Some(source_owned.as_str()),
            evidence_ref: None,
            fields: args.fields,
            verify: args.verify.clone(),
            policy: args.policy.clone(),
            admit_sink: args.admit_sink,
            verify_sink: args.verify_sink,
            timestamp: args.timestamp,
        };
        let decision = gated_store_verified(&mut req, args.candidate, args.signal_source, write)?;
        if !decision.pass {
            continue;
        }
        let published = req.item.clone();
        let item_id = published.id.clone();
        args.leaves.publish(published);
        time_to_queryable_ms = ingest_started.elapsed().as_millis();
        let signal = MaintenanceSignal::leaf_attached(&item_id, args.ctx.bank.as_str());
        maintenance_signals.push(signal);
        admitted.push(AdmittedLeaf {
            item_id,
            decision,
            leaf_queryable: true,
            chunk_ids: unit.chunk_ids.clone(),
        });
    }
    Ok((admitted, maintenance_signals, time_to_queryable_ms))
}

/// Parallel chunk extract → consolidate → gated admit → leaf publish.
///
/// Partial-success (default): verified chunks proceed; failed chunks are logged in
/// `chunk_results` and do not poison successful admits.
/// All-or-nothing: any refuse/error skips admit entirely.
///
/// Maintenance handoff is dispatched to a detached thread after leaf publish
/// (see [`crate::maintenance::dispatch_async`]), so a slow hook never gates the
/// write response. Leaf get uses [`LeafIndex`] and does not wait on ancestor refresh.
///
/// # Errors
/// Bank isolation failures; gated store structural errors.
pub fn parallel_ingest(
    args: &ParallelIngestArgs<'_>,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> Result<IngestOutcome, AmError> {
    bank_guard(args.ctx, &args.item_base)?;
    if args.config.embedding_merge {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "embedding_merge must stay off until dense index exists; deterministic fingerprint is the merge authority",
        ));
    }

    let t0 = Instant::now();
    let profile = args.config.profile_label.as_str();
    let chunks = partition_session(args.turns, args.config.max_turns_per_chunk);
    if chunks.is_empty() {
        return Ok(empty_outcome(profile, 0, vec![], vec![]));
    }

    let t_extract = Instant::now();
    let agg = extract_chunks_parallel(
        args.extractor,
        &chunks,
        &args.schema_template,
        args.fields,
        args.verify,
        args.config.concurrency,
        args.item_base.context.as_deref(),
    );
    let extract_phase_ms = t_extract.elapsed().as_millis();
    let retained: Vec<_> = agg
        .results
        .iter()
        .filter_map(|r| r.retained.clone())
        .collect();

    let abort_all = args.config.all_or_nothing
        && (agg.refused_count > 0 || agg.error_count > 0 || agg.verified_count < chunks.len());
    if abort_all {
        return Ok(empty_outcome(
            profile,
            extract_phase_ms,
            agg.results,
            retained,
        ));
    }

    let verified = to_verified(&agg);
    if verified.is_empty() {
        return Ok(empty_outcome(
            profile,
            extract_phase_ms,
            agg.results,
            retained,
        ));
    }

    let consolidated = consolidate(&verified);
    let (admitted, maintenance_signals, time_to_queryable_ms) =
        admit_canonical_units(args, &consolidated.units, write, t0)?;
    if !admitted.is_empty() {
        args.leaves
            .record_time_to_queryable(args.ctx.bank.as_str(), time_to_queryable_ms);
    }
    // Non-blocking handoff: the hook runs on a detached thread so a slow
    // maintenance consumer can never gate the write response or leaf get. The
    // handle is dropped on purpose here; a caller that must answer over durable
    // rows drains it instead.
    let _ = dispatch_async(args.maintenance.clone(), maintenance_signals.clone());

    Ok(IngestOutcome {
        leaf_queryable: !admitted.is_empty(),
        admitted,
        chunk_results: agg.results,
        retained,
        consolidate_merges: consolidated.stats.merges,
        contradiction_splits: consolidated.stats.contradiction_splits,
        maintenance_signals,
        timing: IngestTiming {
            // Timing fields exclude asynchronous structural maintenance.
            time_to_queryable_ms,
            extract_phase_ms,
            structural_maintenance_ms: 0,
        },
        profile_label: args.config.profile_label.clone(),
    })
}

#[cfg(test)]
#[path = "ingest_tests.rs"]
mod ingest_tests;

#[cfg(test)]
#[path = "ingest_leaf_tests.rs"]
mod ingest_leaf_tests;

#[cfg(test)]
#[path = "ingest_gate_tests.rs"]
mod ingest_gate_tests;
