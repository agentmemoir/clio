// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `persona_put_stable` discrete write tool
//! (categorical/durable attributes with invalidation semantics).
//!
//! ## Owns
//! - Path-split rejection: numeric-intensity payloads and continuous-declared
//!   keys are rejected with a pointer to `persona_observe_preference`
//! - Admission + score stamping on the persona stable entry
//! - Optional `context` validation (fail-closed before any write) and the
//!   context rule handoff: an omitted context is preserved by the store;
//!   a supplied context is the explicit audited replacement act (the closed
//!   prior version row keeps its own sealed context in history)
//!
//! ## Does not own
//! - Persona SQL (`clio-store::PersonaStore`) or the invalidation SQL
//! - Scoring math (`clio-admission`) or EMA math
//!
//! ## Boundary
//!
//! Changes to stable write gating or rejections should normally require changes here.
//! Changes to store SQL or factor formulas should not require changes here.

use crate::budget::truncate_document;
use crate::{PersonaEvent, PersonaSink, require_bank_and_timestamp, require_nonempty};
use clio_admission::gated::{GateRequest, SignalSource, gated_create};
use clio_admission::{AdmissionPolicy, Candidate, Decision, EventSink};
use clio_store::{ContinuousStore, PersonaStore};
use clio_types::{
    AmError, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory, UpdateRule,
    validate_persona_source,
};

/// Inputs for `persona_put_stable`.
pub struct PersonaPutStableArgs<'a> {
    /// Persona repository (discrete write path).
    pub store: &'a dyn PersonaStore,
    /// Continuous repository, used only to reject keys declared continuous.
    pub continuous: &'a dyn ContinuousStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Stable attribute key (e.g. `communication_style`).
    pub key: &'a str,
    /// Categorical value (a numeric-only payload is rejected).
    pub value: &'a str,
    /// Optional confidence in `0.0..=1.0`.
    pub confidence: Option<f64>,
    /// `explicit` | `inferred` (default `explicit`).
    pub source: Option<&'a str>,
    /// Evidence reference (turn id / tool call id).
    pub evidence_ref: Option<&'a str>,
    /// Optional free-text rationale; sealed at rest under the bank DEK and
    /// surfaced by `persona_get`. `persona_put_stable` is the discrete persona
    /// write surface, so supplying a context here on an existing entry IS the
    /// authorized explicit replacement act (audited at the store with
    /// presence/length/hash stats, never raw text); omitting it preserves the
    /// stored context. Validated (`validate_context`) before any
    /// write; an invalid context writes nothing.
    pub context: Option<&'a str>,
    /// Thresholds / weights for the gate.
    pub policy: AdmissionPolicy<'a>,
    /// Neighbor/recency signal source for the gate.
    pub signal_source: &'a dyn SignalSource,
    /// Admission accept/reject telemetry sink.
    pub admission_sink: &'a dyn EventSink,
    /// Persona telemetry sink.
    pub persona_sink: &'a dyn PersonaSink,
    /// Persona token budget for post-mutation truncation.
    pub budget_tokens: usize,
    /// Caller timestamp; doubles as the new version's valid-time start.
    pub timestamp: &'a str,
    /// Score + validate without persisting (batch pre-flight mode).
    pub dry_run: bool,
}

/// Result of `persona_put_stable`.
#[derive(Debug, Clone, PartialEq)]
pub enum PersonaPutStableOutcome {
    /// The entry was admitted, written, and scored.
    Admitted {
        /// The written entry (with its admission score).
        entry: Box<clio_types::PersonaStableEntry>,
        /// The admission decision.
        decision: Decision,
        /// Keys dropped by the post-mutation budget truncation.
        dropped_keys: Vec<String>,
    },
    /// The gate rejected the candidate; nothing was written.
    Rejected {
        /// The failing decision.
        decision: Decision,
    },
    /// Scored and staged without persisting (batch pre-flight); nothing was
    /// written and no budget truncation ran.
    DryRun {
        /// Would-write entry (with its admission score; not persisted).
        entry: Box<clio_types::PersonaStableEntry>,
        /// The passing admission decision.
        decision: Decision,
    },
}

/// Upsert one stable entry under invalidation semantics.
///
/// The prior open version of the same `(bank, key)` is closed on both time
/// axes (history retained); the gates run before any write and the
/// returned admission score is stamped on the stored entry. The persona token
/// budget is enforced after mutation by truncation, never by rejection.
/// Context follows an omitted context is preserved by the store (the
/// prior open version's sealed context is carried into the new version); a
/// supplied context is the explicit audited replacement, with the prior value
/// reconstructable through the closed history row. The returned entry carries
/// the caller-supplied context; the stored (preserved) context is surfaced by
/// `persona_get`.
///
/// # Errors
/// Missing bank, empty key/value, scalar-intensity payload (pointing at
/// `persona_observe_preference`), continuous-declared key, or store failures.
#[allow(clippy::too_many_lines)]
pub fn persona_put_stable(
    args: &PersonaPutStableArgs<'_>,
) -> Result<PersonaPutStableOutcome, AmError> {
    require_bank_and_timestamp(args.ctx, args.timestamp)?;
    require_nonempty(args.key, "key")?;
    require_nonempty(args.value, "value")?;
    if args.value.trim().parse::<f64>().is_ok() {
        return Err(AmError::new(
            clio_types::ErrorCode::WrongUpdateRule,
            format!(
                "value `{}` looks like a scalar intensity; use persona_observe_preference \
                 for continuous preferences",
                args.value.trim()
            ),
        ));
    }
    if let Some(c) = args.confidence.filter(|v| !(0.0..=1.0).contains(v)) {
        return Err(AmError::new(
            clio_types::ErrorCode::OutOfRange,
            format!("confidence must be in 0.0..=1.0, got {c}"),
        ));
    }
    let source = args.source.unwrap_or("explicit").to_owned();
    validate_persona_source(&source)?;
    // Fail closed before any write (including the admission gate's write
    // closure): an invalid context must store nothing.
    if let Some(text) = args.context {
        clio_types::validate_context(text)?;
    }
    if let Some(schema) = args
        .continuous
        .get_attribute_schema(args.ctx.bank.as_str(), args.key)?
        && schema.update_rule == UpdateRule::Continuous
    {
        return Err(AmError::new(
            clio_types::ErrorCode::WrongUpdateRule,
            format!(
                "key `{}` is declared update_rule=continuous; use \
                     persona_observe_preference",
                args.key
            ),
        ));
    }
    let scoring_item = build_scoring_item(args);
    let req = GateRequest {
        item: &scoring_item,
        ctx: args.ctx,
        policy: args.policy.clone(),
        sink: args.admission_sink,
        timestamp: args.timestamp,
    };
    let entry_from = |item: &MemoryItem| clio_types::PersonaStableEntry {
        bank_id: args.ctx.bank.as_str().to_owned(),
        key: args.key.to_owned(),
        value: args.value.to_owned(),
        confidence: args.confidence,
        source: source.clone(),
        source_ref: args
            .evidence_ref
            .filter(|r| !r.is_empty())
            .map(str::to_owned),
        context: args.context.map(str::to_owned),
        admission_score: item.admission_score.unwrap_or_default(),
        as_of: args.timestamp.to_owned(),
        valid_until: None,
    };
    let store = args.store;
    let mut entry = None;
    let mut write = |item: &MemoryItem| {
        let e = entry_from(item);
        if args.dry_run {
            entry = Some(Box::new(e));
            return Ok(());
        }
        store.put_stable_entry(&e, args.ctx.actor.as_str())
    };
    let decision = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        args.signal_source,
        &mut write,
    )?;
    if args.dry_run {
        if let Some(entry) = entry {
            return Ok(PersonaPutStableOutcome::DryRun { entry, decision });
        }
        return Err(AmError::new(
            clio_types::ErrorCode::Internal,
            "persona stable dry run stored nothing",
        ));
    }
    if decision.pass {
        let full = store.get_persona_document(args.ctx.bank.as_str())?;
        let truncation = truncate_document(&full, args.budget_tokens);
        emit(args, "admitted", Some(decision.admission_score), None);
        Ok(PersonaPutStableOutcome::Admitted {
            entry: Box::new(entry_from(&MemoryItem {
                admission_score: Some(decision.admission_score),
                ..build_scoring_item(args)
            })),
            decision,
            dropped_keys: truncation.dropped_keys,
        })
    } else {
        emit(
            args,
            "rejected",
            Some(decision.admission_score),
            decision.rejection_reason.as_deref(),
        );
        Ok(PersonaPutStableOutcome::Rejected { decision })
    }
}

fn emit(args: &PersonaPutStableArgs<'_>, outcome: &str, score: Option<f64>, reason: Option<&str>) {
    args.persona_sink.emit(PersonaEvent {
        operation: "persona_put_stable".into(),
        path: "discrete".into(),
        outcome: outcome.into(),
        bank: args.ctx.bank.as_str().into(),
        actor: args.ctx.actor.as_str().into(),
        key: args.key.into(),
        timestamp: args.timestamp.into(),
        admission_score: score,
        state: None,
        trend: None,
        rejection_reason: reason.map(str::to_owned),
    });
}

/// Build the scoring item for the gate. Never persisted as a store item:
/// persona rows live in the persona tables.
fn build_scoring_item(args: &PersonaPutStableArgs<'_>) -> MemoryItem {
    MemoryItem {
        id: format!("persona-{}", args.key),
        bank_id: args.ctx.bank.as_str().to_owned(),
        subject_id: args.ctx.bank.as_str().to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: args.confidence,
        admission_score: None,
        source_ref: args.evidence_ref.map(str::to_owned),
        snapshot: None,
        gist: Some(format!("{}: {}", args.key, args.value)),
        valid_from: args.timestamp.to_owned(),
        valid_until: None,
        tx_from: args.timestamp.to_owned(),
        tx_until: None,
        created_at: args.timestamp.to_owned(),
        updated_at: args.timestamp.to_owned(),
        context: None,
    }
}

#[cfg(test)]
#[path = "stable_tests.rs"]
pub(crate) mod stable_tests;
