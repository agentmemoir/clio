// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `correct` service entrypoint: validating
//! the mandatory correction reason and routing the correction by the target's
//! declared `update_rule`.
//!
//! ## Owns
//! - Reason/actor validation (blank reasons fail closed)
//! - Discrete routing: supersede + corrected assertion via `Store::correct_item`
//! - Continuous routing: re-center the EMA state with a recorded reason
//! - Authorized belief-context correction: the only mutation path for a
//!   stored belief's item-level `context`, via `correct_belief_context`
//! - Fail-closed rejections: belief VALUE/confidence corrections (beliefs
//!   evolve via `belief_observe` appends), discrete attribute keys,
//!   non-numeric continuous payloads, item `context` supplied on the
//!   continuous/EMA route (which owns no item and no item context)
//!
//! ## Does not own
//! - The atomic correction SQL (`clio-store`)
//! - MCP confirmation gating (`clio-mcp` previews before `confirm`)
//!
//! ## Boundary
//!
//! Changes to correction routing or validation should normally require changes
//! here. Changes to storage SQL or transport gating should not require changes here.
//!
//! Keep this module focused on the correction command service.

use clio_config::secret::scrub_inline_secrets;
use clio_store::{ContinuousStore, CorrectionSpec, PostgresStore, SqliteStore, Store};
use clio_types::{AmError, ContinuousState, EpistemicKind, ErrorCode, MemoryItem, UpdateRule};
use serde::Serialize;

/// Outcome of a routed `correct` call.
#[derive(Debug, Clone, PartialEq)]
pub enum CorrectOutcome {
    /// Discrete item: prior superseded, corrected assertion inserted.
    Discrete(clio_store::CorrectionOutcome),
    /// Continuous attribute: re-centered EMA state.
    Continuous(ContinuousState),
}

/// Serializable result shape for tool bindings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CorrectResult {
    /// Which correction path applied.
    pub rule: &'static str,
    /// Discrete outcome (when `rule = discrete`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction: Option<clio_store::CorrectionOutcome>,
    /// Continuous state after re-centering (when `rule = continuous`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ContinuousState>,
}

/// Inputs for one auditable correction.
pub struct CorrectArgs<'a> {
    /// Discrete repository (item corrections).
    pub store: &'a dyn Store,
    /// Continuous repository (attribute registry + EMA engine).
    pub continuous: &'a dyn ContinuousStore,
    /// Bank isolation key (tenant boundary; required).
    pub bank_id: &'a str,
    /// Attribution actor.
    pub actor: &'a str,
    /// Target: item id (discrete) or declared attribute key (continuous).
    pub item_id: &'a str,
    /// Corrected value; numeric for the continuous path.
    pub new_value: &'a str,
    /// Mandatory non-empty correction reason.
    pub reason: &'a str,
    /// Optional explicit replacement context for item-addressed
    /// corrections. `None` preserves the prior value; `Some` is an audited
    /// replacement, and the prior value stays reconstructable via
    /// `audit_trail`. A declared continuous attribute has no item context, so a
    /// supplied value is rejected there instead of being silently discarded.
    pub context: Option<&'a str>,
    /// Correction timestamp (closes the prior axes; stamps the new rows).
    pub timestamp: &'a str,
    /// Caller-supplied id for the corrected assertion (discrete path only).
    pub new_item_id: &'a str,
}

/// Auditable correction (`correct(item_id, new_value, reason)`).
///
/// Routing mirrors the `update` facade: a declared **continuous** attribute key
/// re-centers the EMA state and stores the reason on the same-transaction
/// audit row; anything else must be an open item, which is superseded on both
/// time axes with the corrected assertion linked via lineage pointers (
/// nothing is deleted). Blank reasons, belief targets (values/confidence stay
/// append-only; their context changes go through [`correct_belief_context`]),
/// discrete attribute keys, and non-numeric continuous payloads fail closed.
///
/// # Errors
/// Missing bank/target/reason, `item_not_found`, belief or wrong-rule targets,
/// or store failures.
pub fn correct(args: &CorrectArgs<'_>) -> Result<CorrectResult, AmError> {
    if args.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item_id is required",
        ));
    }
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "reason is required and must be non-empty",
        ));
    }
    let masked_reason = scrub_inline_secrets(reason);
    if let Some(schema) = args
        .continuous
        .get_attribute_schema(args.bank_id, args.item_id)?
    {
        return route_declared(args, schema.update_rule, &masked_reason);
    }
    route_discrete(args, &masked_reason)
}

/// Target is a declared attribute: rule decides the legal path.
fn route_declared(
    args: &CorrectArgs<'_>,
    update_rule: UpdateRule,
    reason: &str,
) -> Result<CorrectResult, AmError> {
    match update_rule {
        UpdateRule::Continuous => {
            // The EMA state is not an item row: it has no `context` field to
            // replace. Fail closed so a supplied value is never silently
            // dropped (preservation rule).
            if args.context.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "attribute `{}` is re-centered through its EMA state, which carries no \
                         item context; omit `context` for a continuous correction",
                        args.item_id
                    ),
                ));
            }
            let value = parse_numeric(args.new_value)?;
            let state = args.continuous.observe_continuous_with_reason(
                args.bank_id,
                args.item_id,
                value,
                args.timestamp,
                args.actor,
                Some(reason),
            )?;
            Ok(CorrectResult {
                rule: "continuous",
                correction: None,
                state: Some(state),
            })
        }
        UpdateRule::Discrete => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "attribute `{}` is declared discrete; corrections are item-addressed — \
                 pass the item id (attribute keys name bank-wide predicates)",
                args.item_id
            ),
        )),
    }
}

/// No declared attribute: the target must be an open item row.
fn route_discrete(args: &CorrectArgs<'_>, masked_reason: &str) -> Result<CorrectResult, AmError> {
    let prior = args
        .store
        .get_memory_item(args.bank_id, args.item_id)?
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::ItemNotFound,
                format!("item `{}` not found in bank", args.item_id),
            )
        })?;
    if prior.epistemic_kind == EpistemicKind::Belief {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "belief targets evolve via belief_observe appends; correct is for \
             item-addressed records",
        ));
    }
    if prior.tx_until.is_some() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "item `{}` is already superseded; correct the active revision",
                args.item_id
            ),
        ));
    }
    if let Some(context) = args.context {
        clio_types::validate_context(context)?;
    }
    let new_item = build_corrected_item(
        &prior,
        args.new_value,
        args.context,
        args.new_item_id,
        args.timestamp,
    );
    let spec = CorrectionSpec {
        bank_id: args.bank_id.to_owned(),
        prior_item_id: args.item_id.to_owned(),
        new_item,
        reason: masked_reason.to_owned(),
        actor: args.actor.to_owned(),
        timestamp: args.timestamp.to_owned(),
    };
    let outcome = args.store.correct_item(&spec)?;
    Ok(CorrectResult {
        rule: "discrete",
        correction: Some(outcome),
        state: None,
    })
}

/// Store side of the authorized belief-context correction.
///
/// Implemented for the concrete `clio-store` backends, which own the write
/// (validate, seal, replace, audit) as an inherent method. The shared
/// `BeliefStore` trait itself stays append/read-only for `belief_observe`;
/// widening it would require touching its per-backend impl blocks, so this
/// narrow local trait keeps the correction routing backend-agnostic.
pub trait BeliefContextCorrector: Send + Sync {
    /// Seal and persist the corrected context plus its audit row.
    ///
    /// # Errors
    /// Validation, unknown belief in bank, crypto, or storage failures.
    fn correct_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError>;
}

impl BeliefContextCorrector for SqliteStore {
    fn correct_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError> {
        SqliteStore::correct_belief_context(self, bank_id, belief_id, new_context, reason, actor)
    }
}

impl BeliefContextCorrector for PostgresStore {
    fn correct_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError> {
        PostgresStore::correct_belief_context(self, bank_id, belief_id, new_context, reason, actor)
    }
}

/// Inputs for an authorized belief-context correction.
pub struct BeliefContextCorrectArgs<'a> {
    /// Belief repository side of the correction (concrete backends).
    pub beliefs: &'a dyn BeliefContextCorrector,
    /// Bank isolation key (tenant boundary; required).
    pub bank_id: &'a str,
    /// Target belief id.
    pub belief_id: &'a str,
    /// Corrected item-level context (required; validated with
    /// `validate_context` before any write).
    pub new_context: &'a str,
    /// Mandatory non-empty correction reason.
    pub reason: &'a str,
    /// Attribution actor.
    pub actor: &'a str,
}

/// Authorized correction of a belief's item-level `context`.
///
/// Belief targets are not items: their values and confidences evolve only via
/// `belief_observe` appends and stay unreachable from [`correct`]. This path
/// changes the sealed item-level `context` only. The store validates and
/// seals the new value and writes an `audit_events` row (operation `correct`)
/// carrying the scrubbed reason and hash-only prior/new context stats
/// (presence, length, hash). The prior sealed value is never retained in the
/// append-only audit, so `audit_trail` shows that a prior context
/// existed and how it hashed, never its content; raw values are available only
/// through an authorized read of the live row.
///
/// # Errors
/// Missing bank/target/reason, invalid context, unknown belief in bank, or
/// store failures.
pub fn correct_belief_context(
    args: &BeliefContextCorrectArgs<'_>,
) -> Result<CorrectResult, AmError> {
    if args.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.belief_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "belief_id is required",
        ));
    }
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "reason is required and must be non-empty",
        ));
    }
    clio_types::validate_context(args.new_context)?;
    let masked_reason = scrub_inline_secrets(reason);
    args.beliefs.correct_context(
        args.bank_id,
        args.belief_id,
        args.new_context,
        &masked_reason,
        args.actor,
    )?;
    Ok(CorrectResult {
        rule: "belief_context",
        correction: None,
        state: None,
    })
}

/// Build the corrected assertion from the prior revision plus `new_value`.
///
/// `new_value` always replaces the non-authoritative gist; when it parses as a
/// JSON object it also becomes the authoritative snapshot (the prior snapshot
/// is retained otherwise). Create-time admission score and provenance are
/// carried over untouched — a correction never re-scores (mutators).
/// Context is carried over unless the caller supplied an explicit replacement.
fn build_corrected_item(
    prior: &MemoryItem,
    new_value: &str,
    new_context: Option<&str>,
    new_item_id: &str,
    timestamp: &str,
) -> MemoryItem {
    let snapshot = match serde_json::from_str::<serde_json::Value>(new_value) {
        Ok(v) if v.is_object() => Some(v),
        _ => prior.snapshot.clone(),
    };
    MemoryItem {
        id: new_item_id.to_owned(),
        bank_id: prior.bank_id.clone(),
        subject_id: prior.subject_id.clone(),
        kind: prior.kind,
        category: prior.category,
        episodic_type: prior.episodic_type,
        epistemic_kind: prior.epistemic_kind,
        source_type: prior.source_type,
        confidence: prior.confidence,
        admission_score: prior.admission_score,
        source_ref: prior.source_ref.clone(),
        context: new_context.map_or_else(|| prior.context.clone(), |c| Some(c.to_owned())),
        snapshot,
        gist: Some(new_value.to_owned()),
        valid_from: timestamp.to_owned(),
        valid_until: None,
        tx_from: timestamp.to_owned(),
        tx_until: None,
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
    }
}

/// Parse the observed value at the trust boundary; no silent coercion.
///
/// # Errors
/// Non-numeric or non-finite payload (fails closed on the continuous path).
fn parse_numeric(raw: &str) -> Result<f64, AmError> {
    let value: f64 = raw.trim().parse().map_err(|_| {
        AmError::new(
            ErrorCode::InvalidType,
            format!("continuous correction must be a finite number, got `{raw}`"),
        )
    })?;
    if !value.is_finite() {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "continuous correction must be finite (NaN/Inf rejected)",
        ));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "correct_tests.rs"]
mod correct_tests;

#[cfg(test)]
#[path = "correct_context_tests.rs"]
pub(crate) mod correct_context_tests;
