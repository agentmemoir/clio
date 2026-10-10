// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `belief_observe` create-vs-append tool.
//!
//! ## Owns
//! - Argument validation, identity resolution, and the create/append branches
//! - The append context rule: omitted or byte-identical only; a
//!   differing value is rejected before any write
//! - Belief observe telemetry events (`created` / `appended` / `rejected`)
//!
//! ## Does not own
//! - Belief persistence (`clio-store`) or scoring math (`signals`)
//! - History reads (`history`) or response mapping (`clio-types::read`)
//!
//! ## Boundary
//!
//! Changes to observe gating or branch order should normally require changes here.
//! Changes to belief storage should not require changes here.
//!
//! Keep this module focused on the `belief_observe` tool.

use crate::signals::{BeliefSignalSource, score_new_belief};
use clio_admission::{AdmissionPolicy, Decision};
use clio_store::BeliefStore;
use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, ErrorCode, OpContext, SourceType, check_confidence,
    proposition_key, validate_context,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic suffix so two observes sharing one caller timestamp cannot collide.
static BELIEF_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_belief_id(bank_id: &str, timestamp: &str) -> String {
    let seq = BELIEF_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("bel-{bank_id}-{timestamp}-{nanos}-{seq}")
}

/// Telemetry for `belief_observe` (spirit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeliefEvent {
    /// Always `belief_observe`.
    pub operation: String,
    /// `created` | `appended` | `rejected`.
    pub outcome: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// Belief id (empty on reject).
    pub belief_id: String,
    /// Normalized proposition identity key.
    pub proposition_key: String,
    /// Trajectory length after the observe.
    pub entry_count: usize,
    /// Observed confidence.
    pub confidence: f64,
    /// Observed provenance.
    pub source_type: String,
    /// Admission score on create.
    pub admission_score: Option<f64>,
    /// Reject reason when `outcome = rejected`.
    pub rejection_reason: Option<String>,
}

/// Sink for belief observe telemetry.
pub trait BeliefSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: BeliefEvent);
}

/// In-memory sink for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct BeliefVecSink {
    events: Mutex<Vec<BeliefEvent>>,
}

impl BeliefVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<BeliefEvent> {
        self.events.lock().expect("belief sink poisoned").clone()
    }
}

impl BeliefSink for BeliefVecSink {
    fn emit(&self, event: BeliefEvent) {
        self.events
            .lock()
            .expect("belief sink poisoned")
            .push(event);
    }
}

/// Inputs for `belief_observe`.
#[derive(Debug)]
pub struct BeliefObserveArgs<'a> {
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Data-subject id for DEK selection.
    pub subject_id: &'a str,
    /// Proposition text.
    pub proposition: &'a str,
    /// Observed confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Evidence reference for this observation.
    pub evidence_ref: &'a str,
    /// Optional bounded descriptive context for the belief. Validated
    /// with `clio_types::validate_context` before any write. On create it is
    /// stored sealed; on append it MUST be omitted or byte-identical to the
    /// stored value — a differing value is a usage error.
    pub context: Option<&'a str>,
    /// `user_stated` | `agent_inferred` | `third_party` (validated).
    pub source_type: &'a str,
    /// Observation time; defaults to `timestamp`.
    pub as_of: Option<&'a str>,
    /// Threshold / weights for the create path.
    pub policy: AdmissionPolicy<'a>,
    /// Telemetry sink.
    pub sink: &'a dyn BeliefSink,
    /// Event / transaction timestamp.
    pub timestamp: &'a str,
}

/// Result of `belief_observe`.
// `Created` necessarily carries a whole `BeliefObject` (identity + full
// first trajectory) while the other variants are telemetry-sized; boxing the
// payload would churn every consumer for no measurable gain, so the size
// warning is accepted here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum BeliefObserveOutcome {
    /// A new belief identity was admitted and created.
    Created {
        /// The created belief (with its first history entry).
        belief: BeliefObject,
        /// The create-path admission decision.
        decision: Decision,
    },
    /// Evidence was appended to an existing belief; prior entries are untouched.
    Appended {
        /// Existing belief id.
        belief_id: String,
        /// Trajectory length after the append.
        entry_count: usize,
        /// Confidence just observed.
        confidence: f64,
        /// Provenance just observed.
        source_type: SourceType,
    },
    /// The create path failed admission; nothing was written.
    Rejected {
        /// The failing decision.
        decision: Decision,
    },
}

#[allow(clippy::too_many_arguments)]
fn emit(
    sink: &dyn BeliefSink,
    args: &BeliefObserveArgs<'_>,
    outcome: &str,
    belief_id: &str,
    key: &str,
    entry_count: usize,
    admission_score: Option<f64>,
    rejection_reason: Option<String>,
) {
    sink.emit(BeliefEvent {
        operation: "belief_observe".into(),
        outcome: outcome.into(),
        bank: args.ctx.bank.as_str().into(),
        actor: args.ctx.actor.as_str().into(),
        timestamp: args.timestamp.into(),
        belief_id: belief_id.into(),
        proposition_key: key.into(),
        entry_count,
        confidence: args.confidence,
        source_type: args.source_type.into(),
        admission_score,
        rejection_reason,
    });
}

/// Append evidence: create the belief identity (gated) or append (validated only).
///
/// Identity is `(bank, normalized proposition)`; a duplicate observe of the same
/// evidence **appends** rather than deduplicating (documented append-only choice).
/// The create path runs five-factor admission with no category gate; the append
/// path only validates and never re-scores novelty/utility.
/// On append the supplied `context` must be omitted or byte-identical to the
/// stored item-level value; a differing value is a usage error and nothing is
/// written.
/// Concurrent first observes serialize: the create-race loser appends rather
/// than surfacing a duplicate-key error.
///
/// # Errors
/// Missing bank, invalid enums/ranges, unavailable signal backends, or store failures.
pub fn belief_observe(
    store: &dyn BeliefStore,
    source: &dyn BeliefSignalSource,
    args: &BeliefObserveArgs<'_>,
) -> Result<BeliefObserveOutcome, AmError> {
    let actor = args.ctx.actor.as_str();
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.timestamp.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "timestamp is required",
        ));
    }
    require_nonempty(args.proposition, "proposition")?;
    require_nonempty(args.evidence_ref, "evidence_ref")?;
    require_nonempty(args.subject_id, "subject_id")?;
    if let Some(context) = args.context {
        validate_context(context)?;
    }
    let source_type = SourceType::parse(args.source_type)?;
    check_confidence(args.confidence)?;
    let as_of = args.as_of.unwrap_or(args.timestamp);
    let key = proposition_key(args.proposition);
    if key.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "proposition normalizes to an empty identity key",
        ));
    }
    let entry = ConfidenceEntry::new(as_of, args.confidence, args.evidence_ref, source_type)?;
    let bank = args.ctx.bank.as_str();

    if let Some(existing) = store.find_belief_by_proposition(bank, &key)? {
        return append_to_existing(store, args, &existing, &key, &entry, source_type);
    }

    let decision = score_new_belief(bank, args.proposition, source_type, source, &args.policy)?;
    if !decision.pass {
        let reason = decision.rejection_reason.clone();
        emit(
            args.sink,
            args,
            "rejected",
            "",
            &key,
            0,
            Some(decision.admission_score),
            reason,
        );
        return Ok(BeliefObserveOutcome::Rejected { decision });
    }
    let belief = BeliefObject {
        id: unique_belief_id(bank, args.timestamp),
        bank_id: bank.into(),
        subject_id: args.subject_id.into(),
        proposition: args.proposition.into(),
        context: args.context.map(str::to_owned),
        // Store-derived at insert never accepted from a caller.
        context_as_of: None,
        confidence_history: vec![entry.clone()],
        created_at: args.timestamp.into(),
        updated_at: as_of.into(),
    };
    if let Err(create_err) = store.create_belief(&belief, actor) {
        // Concurrent first observes: the loser of the create race re-resolves the
        // identity (the winner committed it) and serializes as an append instead
        // of surfacing a duplicate-key error. Any other failure propagates.
        let existing = store.find_belief_by_proposition(bank, &key)?;
        if let Some(existing) = existing {
            return append_to_existing(store, args, &existing, &key, &entry, source_type);
        }
        return Err(create_err);
    }
    emit(
        args.sink,
        args,
        "created",
        &belief.id,
        &key,
        1,
        Some(decision.admission_score),
        None,
    );
    Ok(BeliefObserveOutcome::Created { belief, decision })
}

fn require_nonempty(value: &str, label: &str) -> Result<(), AmError> {
    if value.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

/// Append to an already-resolved belief and emit `appended` telemetry.
///
/// The supplied `context` MUST be omitted or byte-identical to the
/// stored item-level value. A differing value (including supplying one when
/// the belief has none) is a usage error: nothing is written, the prior
/// context stays untouched, and the rejection is emitted as `rejected`
/// telemetry following the belief audit pattern. An omitted or byte-identical
/// context appends normally; the store call passes `None` so the sealed value
/// is preserved rather than re-encrypted.
///
/// The stored context is compared as the opened plaintext. When the stored
/// context is unreadable (for example a crypto-shredded subject) its identity
/// cannot be proven, so a supplied value fails closed.
fn append_to_existing(
    store: &dyn BeliefStore,
    args: &BeliefObserveArgs<'_>,
    existing: &BeliefObject,
    key: &str,
    entry: &ConfidenceEntry,
    source_type: SourceType,
) -> Result<BeliefObserveOutcome, AmError> {
    if let Some(supplied) = args.context
        && existing.context.as_deref() != Some(supplied)
    {
        let reason = "belief context differs from the stored value; changing a \
                          belief's context requires the authorized correction path \
                          (correct_belief_context)"
            .to_owned();
        emit(
            args.sink,
            args,
            "rejected",
            &existing.id,
            key,
            existing.confidence_history.len(),
            None,
            Some(reason.clone()),
        );
        return Err(AmError::new(ErrorCode::InvalidArgument, reason));
    }
    store.append_confidence(
        args.ctx.bank.as_str(),
        &existing.id,
        entry,
        args.ctx.actor.as_str(),
    )?;
    let entry_count = store
        .belief_history(args.ctx.bank.as_str(), &existing.id, None)?
        .len();
    emit(
        args.sink,
        args,
        "appended",
        &existing.id,
        key,
        entry_count,
        None,
        None,
    );
    Ok(BeliefObserveOutcome::Appended {
        belief_id: existing.id.clone(),
        entry_count,
        confidence: entry.confidence,
        source_type,
    })
}

#[cfg(test)]
#[path = "observe_context_tests.rs"]
mod observe_context_tests;

#[cfg(test)]
#[path = "observe_tests.rs"]
pub(crate) mod observe_tests;

#[cfg(test)]
#[path = "observe_flow_tests.rs"]
pub(crate) mod observe_flow_tests;

#[cfg(test)]
#[path = "observe_read_tests.rs"]
mod observe_read_tests;
