// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `persona_observe_preference`: the thin
//! continuous adapter over the shared EMA engine.
//!
//! ## Owns
//! - Numeric-only preference observations routed through
//!   `clio_write::persona::observe_continuous` (never a second EMA copy)
//! - Attribute registration for preference keys + create-time score stamping
//! - Optional `context` validation (fail-closed before any write) and the
//!   context rule handoff: an omitted context is preserved by the store;
//!   a supplied context is the explicit audited replacement act (the store
//!   records hash-only context statistics in the audit detail and never
//!   retains the prior sealed ciphertext)
//!
//! ## Does not own
//! - EMA formulas (`clio-types::apply_observation` behind the store)
//! - Persona SQL (`clio-store::PersonaStore`) or scoring math (`clio-admission`)
//!
//! ## Boundary
//!
//! Changes to adapter wiring or score-stamp rules should normally require
//! changes here. Changes to the engine math should not (`clio-types::attribute`).
//!
//! Keep this module focused on the thin continuous-observe adapter tool.

use crate::budget::truncate_document;
use crate::{PersonaEvent, PersonaSink, require_bank_and_timestamp};
use clio_admission::gated::{GateRequest, SignalSource, gated_create};
use clio_admission::{AdmissionPolicy, Candidate, Decision, EventSink};
use clio_store::{ContinuousStore, PersonaStore};
use clio_types::{
    AmError, AttributeSchema, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
    UpdateRule, validate_persona_source,
};

/// Inputs for `persona_observe_preference`.
pub struct PersonaObservePreferenceArgs<'a> {
    /// Persona repository (preference rows).
    pub store: &'a dyn PersonaStore,
    /// Continuous repository (attribute registry + EMA engine).
    pub continuous: &'a dyn ContinuousStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Preference key (e.g. `brevity_affinity`).
    pub key: &'a str,
    /// Observed scalar intensity (finite; NaN/Inf rejected).
    pub observed_value: f64,
    /// Optional confidence in `0.0..=1.0`.
    pub confidence: Option<f64>,
    /// `explicit` | `inferred` (default `inferred`).
    pub source: Option<&'a str>,
    /// Evidence reference (turn id / tool call id).
    pub evidence_ref: Option<&'a str>,
    /// Optional free-text rationale; sealed at rest under the bank DEK and
    /// surfaced by `persona_get`. When omitted on an existing preference, its
    /// stored context is preserved (ordinary updates never clear it).
    /// Supplying a new context is the explicit audited replacement act: the
    /// store persists it and records hash-only prior/effective context stats
    /// (never the raw text, never a sealed copy, so erasure leaves no derived
    /// copy behind). Validated before any write.
    pub context: Option<&'a str>,
    /// Thresholds / weights for the create-path gate.
    pub policy: AdmissionPolicy<'a>,
    /// Neighbor/recency signal source for the gate.
    pub signal_source: &'a dyn SignalSource,
    /// Admission accept/reject telemetry sink.
    pub admission_sink: &'a dyn EventSink,
    /// Continuous mutator telemetry sink (`clio-write` contract).
    pub continuous_sink: &'a dyn clio_write::update::ContinuousSink,
    /// Persona telemetry sink.
    pub persona_sink: &'a dyn PersonaSink,
    /// Persona token budget for post-mutation truncation.
    pub budget_tokens: usize,
    /// Observation timestamp.
    pub timestamp: &'a str,
}

/// Result of `persona_observe_preference`.
#[derive(Debug, Clone, PartialEq)]
pub enum PersonaObserveOutcome {
    /// The observation was applied through the shared engine.
    Observed {
        /// EMA state after the observation.
        state: f64,
        /// Trend after the observation.
        trend: f64,
        /// Create-time admission score (retained on later observes).
        admission_score: f64,
        /// Keys dropped by the post-mutation budget truncation.
        dropped_keys: Vec<String>,
    },
    /// The create path failed admission; nothing was written.
    Rejected {
        /// The failing decision.
        decision: Decision,
    },
}

/// EMA-update one continuous preference.
///
/// A continuous signal drifts rather than flipping, so it is smoothed instead of
/// overwritten: one strong observation nudges the value without erasing what the
/// earlier ones said.
///
/// Delegates to the shared continuous engine with a declared
/// `update_rule=continuous` attribute for the key; the attribute is registered
/// only after the create gate passes (a rejected candidate writes
/// nothing, including the attribute declaration). On first create the key runs
/// admission and the score is stamped; later observes retain it.
/// Concurrent observes on the same key serialize inside the engine's
/// transactional RMW.
///
/// # Errors
/// Missing bank, non-finite value, discrete-declared key (pointing at
/// `persona_put_stable`), or store failures.
pub fn persona_observe_preference(
    args: &PersonaObservePreferenceArgs<'_>,
) -> Result<PersonaObserveOutcome, AmError> {
    require_bank_and_timestamp(args.ctx, args.timestamp)?;
    crate::require_nonempty(args.key, "key")?;
    if !args.observed_value.is_finite() {
        return Err(AmError::new(
            clio_types::ErrorCode::OutOfRange,
            "observed_value must be a finite number (NaN/Inf rejected)",
        ));
    }
    if let Some(c) = args.confidence.filter(|v| !(0.0..=1.0).contains(v)) {
        return Err(AmError::new(
            clio_types::ErrorCode::OutOfRange,
            format!("confidence must be in 0.0..=1.0, got {c}"),
        ));
    }
    let source = args.source.unwrap_or("inferred").to_owned();
    validate_persona_source(&source)?;
    // Fail closed before any write (including the create gate and the
    // attribute declaration): an invalid context must store nothing.
    if let Some(text) = args.context {
        clio_types::validate_context(text)?;
    }
    let bank = args.ctx.bank.as_str();
    let declared = args.continuous.get_attribute_schema(bank, args.key)?;
    if declared
        .as_ref()
        .is_some_and(|s| s.update_rule == UpdateRule::Discrete)
    {
        return Err(AmError::new(
            clio_types::ErrorCode::WrongUpdateRule,
            format!(
                "key `{}` is declared update_rule=discrete; use persona_put_stable \
                 for categorical stables",
                args.key
            ),
        ));
    }
    let existing = args.store.get_preference(bank, args.key)?;
    let admission_score = if let Some(pref) = existing {
        pref.admission_score
    } else {
        let decision = run_create_admission(args)?;
        if !decision.pass {
            emit(
                args,
                "rejected",
                Some(decision.admission_score),
                None,
                None,
                Some(&decision),
            );
            return Ok(PersonaObserveOutcome::Rejected { decision });
        }
        decision.admission_score
    };
    // Registered only after the gate passes: a rejected create must write
    // nothing, including the attribute declaration.
    ensure_continuous_declaration(args, declared.as_ref())?;
    let as_of = args.timestamp;
    let state = clio_write::persona::observe_continuous(
        args.continuous,
        args.ctx,
        args.key,
        args.observed_value,
        as_of,
        args.continuous_sink,
    )?;
    let pref = clio_types::PersonaPreference {
        bank_id: bank.to_owned(),
        key: args.key.to_owned(),
        value: state.state,
        trend: state.trend,
        confidence: args.confidence,
        source,
        source_ref: args
            .evidence_ref
            .filter(|r| !r.is_empty())
            .map(str::to_owned),
        // The store preserves the row's stored context when this is `None`.
        context: args.context.map(str::to_owned),
        admission_score,
        as_of: state.as_of.clone(),
    };
    args.store
        .put_preference_state(&pref, args.ctx.actor.as_str())?;
    let full = args.store.get_persona_document(bank)?;
    let truncation = truncate_document(&full, args.budget_tokens);
    emit(
        args,
        "observed",
        Some(admission_score),
        Some(state.state),
        Some(state.trend),
        None,
    );
    Ok(PersonaObserveOutcome::Observed {
        state: state.state,
        trend: state.trend,
        admission_score,
        dropped_keys: truncation.dropped_keys,
    })
}

fn emit(
    args: &PersonaObservePreferenceArgs<'_>,
    outcome: &str,
    score: Option<f64>,
    state: Option<f64>,
    trend: Option<f64>,
    decision: Option<&Decision>,
) {
    args.persona_sink.emit(PersonaEvent {
        operation: "persona_observe_preference".into(),
        path: "continuous".into(),
        outcome: outcome.into(),
        bank: args.ctx.bank.as_str().into(),
        actor: args.ctx.actor.as_str().into(),
        key: args.key.into(),
        timestamp: args.timestamp.into(),
        admission_score: score,
        state,
        trend,
        rejection_reason: decision.and_then(|d| d.rejection_reason.clone()),
    });
}

/// Register the continuous declaration when the key is undeclared (declaration
/// is immutable per key). Called only after the create gate passes: a rejected
/// candidate must write nothing.
///
/// # Errors
/// Registration failure (with the concurrent first-observe race resolved
/// against the winner's declaration).
fn ensure_continuous_declaration(
    args: &PersonaObservePreferenceArgs<'_>,
    declared: Option<&AttributeSchema>,
) -> Result<(), AmError> {
    let bank = args.ctx.bank.as_str();
    if declared.is_some() {
        return Ok(());
    }
    let declaration = AttributeSchema {
        bank_id: bank.to_owned(),
        attr_key: args.key.to_owned(),
        update_rule: UpdateRule::Continuous,
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: args.timestamp.to_owned(),
        updated_at: None,
    };
    let actor = args.ctx.actor.as_str();
    match args.continuous.put_attribute_schema(&declaration, actor) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Concurrent first observes: the winner registered the key;
            // the loser proceeds against the existing declaration.
            match args.continuous.get_attribute_schema(bank, args.key)? {
                Some(_) => Ok(()),
                None => Err(first),
            }
        }
    }
}

/// Create-time admission for a new preference key (scored, no category
/// bypass — the persona category applies to companion writes).
fn run_create_admission(args: &PersonaObservePreferenceArgs<'_>) -> Result<Decision, AmError> {
    let item = MemoryItem {
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
        gist: Some(format!("{}: {:.4}", args.key, args.observed_value)),
        valid_from: args.timestamp.to_owned(),
        valid_until: None,
        tx_from: args.timestamp.to_owned(),
        tx_until: None,
        created_at: args.timestamp.to_owned(),
        updated_at: args.timestamp.to_owned(),
        context: None,
    };
    let req = GateRequest {
        item: &item,
        ctx: args.ctx,
        policy: args.policy.clone(),
        sink: args.admission_sink,
        timestamp: args.timestamp,
    };
    let mut write = |_item: &MemoryItem| Ok::<(), AmError>(());
    gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        args.signal_source,
        &mut write,
    )
}

#[cfg(test)]
#[path = "observe_tests.rs"]
mod observe_tests;
