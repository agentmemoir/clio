// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the generic `update(item_id, new_value,
//! update_rule)` rule splitter and continuous mutator telemetry.
//!
//! ## Owns
//! - `update` routing: `discrete` → supersede-and-store path, `continuous` → EMA engine
//! - `wrong_update_rule` rejection in both directions and for `fact`/`belief`
//! - `ContinuousEvent` / `ContinuousSink` telemetry on successful mutates
//!
//! ## Does not own
//! - EMA math (shared engine in `clio-types` behind the store) or SQL drivers
//! - Admission scoring (mutators are N/A-gated; audit still emitted)
//!
//! ## Boundary
//!
//! Changes to update routing or rejection semantics should normally require changes here.
//! Changes to store SQL or formula math should not require changes here.
//!
//! Keep this module focused on the `update` rule splitter and continuous mutator telemetry.

use crate::triple::{TripleEvent, TripleSink};
use clio_store::{ContinuousStore, CorrectionSpec, Store};
use clio_types::{
    AmError, ContinuousState, EpistemicKind, ErrorCode, MemoryItem, OpContext, TripleEndOutcome,
    UpdateRule,
};
use serde::{Deserialize, Serialize};

/// Reason recorded on the correction audit rows a discrete update writes; an
/// update carries no caller reason of its own.
const SUPERSEDE_REASON: &str = "update";

/// Telemetry for continuous mutators (spirit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousEvent {
    /// `observe_continuous` | `update_continuous`.
    pub operation: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Attribute key observed.
    pub attr_key: String,
    /// Observation timestamp (stamped on the applied observation).
    pub timestamp: String,
    /// EMA state after the update.
    pub state: f64,
    /// Trend after the update.
    pub trend: f64,
}

/// Sink for continuous mutator telemetry.
pub trait ContinuousSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: ContinuousEvent);
}

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct ContinuousVecSink {
    events: std::sync::Mutex<Vec<ContinuousEvent>>,
}

impl ContinuousVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<ContinuousEvent> {
        self.events
            .lock()
            .expect("continuous sink poisoned")
            .clone()
    }
}

impl ContinuousSink for ContinuousVecSink {
    fn emit(&self, event: ContinuousEvent) {
        self.events
            .lock()
            .expect("continuous sink poisoned")
            .push(event);
    }
}

/// Outcome of a routed `update` call.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateOutcome {
    /// Continuous attribute: new EMA state (no invalidation).
    Continuous(ContinuousState),
    /// Discrete record: both time axes closed (invalidation / supersession).
    Discrete(TripleEndOutcome),
}

/// Inputs for the shared `update` entrypoint.
pub struct UpdateArgs<'a> {
    /// Discrete repository (invalidation path).
    pub store: &'a dyn Store,
    /// Continuous repository (attribute registry + EMA engine).
    pub continuous: &'a dyn ContinuousStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Update-rule-scoped address. `update_rule=continuous`: the declared
    /// attribute key. `update_rule=discrete`: the item id to supersede.
    pub target: &'a str,
    /// New value; parsed as a finite float on the continuous path, and stored
    /// as the replacement item's value on the discrete one.
    pub new_value: &'a str,
    /// Which rule the caller wants to apply.
    pub update_rule: UpdateRule,
    /// Caller-supplied id for the replacement item (discrete path only).
    pub new_item_id: &'a str,
    /// Observation / invalidation timestamp.
    pub timestamp: &'a str,
    /// Continuous mutator telemetry sink.
    pub continuous_sink: &'a dyn ContinuousSink,
    /// Discrete (invalidate) telemetry sink.
    pub triple_sink: &'a dyn TripleSink,
}

/// Shared `update` facade: one entrypoint, two legal paths.
///
/// ## Addressability contract
///
/// `target` is scoped by `update_rule` because the two rule domains are
/// addressed by different identifiers:
/// - `update_rule=continuous` → `target` is a **declared attribute key**. The
///   continuous series is bank-scoped by `(bank, attr_key)`, so the key is the
///   stable address.
/// - `update_rule=discrete` → `target` is an **item id** to invalidate. A
///   declared discrete attribute key is not accepted here: a key names a
///   bank-wide predicate shared by many subjects, so invalidating "by key"
///   would close unrelated subjects' edges. The item id is the only
///   unambiguous address.
///
/// Resolution order is deliberate: a `target` matching a registered attribute is
/// treated as an attribute key first. Generated item ids (`itm-…`) never collide
/// with attribute keys in practice. Passing an attribute key to the discrete
/// path fails as `InvalidArgument`; passing an item id to the continuous path
/// fails as `WrongUpdateRule` (no registration for it).
///
/// Routing:
/// - `update_rule=continuous` → target must be a registered continuous
///   attribute key; routes to the shared EMA engine. Never invalidates.
/// - `update_rule=discrete` → target must be an item id; routes to the
///   supersede-and-store path (both axes closed, never EMA), which writes
///   `new_value` onto a replacement item the old revision links to.
/// - Any `fact`/`belief`/unknown wire value fails at `UpdateRule::parse`
///   with `wrong_update_rule` before reaching this function.
///
/// # Errors
/// `wrong_update_rule` on every rule mismatch (no mutation), non-numeric
/// payloads, missing bank, or store failures.
pub fn update(args: &UpdateArgs<'_>) -> Result<UpdateOutcome, AmError> {
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    match args
        .continuous
        .get_attribute_schema(args.ctx.bank.as_str(), args.target)?
    {
        Some(schema) => route_declared(args, &schema),
        None => route_unregistered(args),
    }
}

/// Target is a declared attribute: the declaration wins over the caller rule.
fn route_declared(
    args: &UpdateArgs<'_>,
    schema: &clio_types::AttributeSchema,
) -> Result<UpdateOutcome, AmError> {
    if schema.update_rule != args.update_rule {
        return Err(AmError::new(
            ErrorCode::WrongUpdateRule,
            format!(
                "attribute `{}` declares update_rule={}; update_rule={} is rejected",
                schema.attr_key,
                schema.update_rule.as_str(),
                args.update_rule.as_str()
            ),
        ));
    }
    match schema.update_rule {
        UpdateRule::Continuous => {
            let value = parse_numeric(args.new_value)?;
            let state = crate::persona::observe_continuous(
                args.continuous,
                args.ctx,
                args.target,
                value,
                args.timestamp,
                args.continuous_sink,
            )?;
            Ok(UpdateOutcome::Continuous(state))
        }
        UpdateRule::Discrete => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "attribute `{}` is discrete but discrete updates are item-addressed \
                 (a bank-wide key is ambiguous across subjects); pass the item id, \
                 or assert/retract with triple_add/triple_end",
                schema.attr_key
            ),
        )),
    }
}

/// Target is not a declared attribute: continuous fails closed, discrete
/// routes to the invalidation path.
fn route_unregistered(args: &UpdateArgs<'_>) -> Result<UpdateOutcome, AmError> {
    match args.update_rule {
        UpdateRule::Continuous => Err(AmError::new(
            ErrorCode::WrongUpdateRule,
            format!(
                "target `{}` has no continuous `update_rule` declaration; \
                 continuous updates require a registered continuous attribute",
                args.target
            ),
        )),
        UpdateRule::Discrete => Ok(UpdateOutcome::Discrete(supersede(args)?)),
    }
}

/// Supersede the target and store `new_value` on the replacement item.
///
/// One `Store::correct_item` transaction closes the prior revision and inserts
/// the replacement as a corrected assertion, so the value a discrete update was
/// given is persisted rather than dropped. A prior that is already closed means
/// nothing was written, which fails closed instead of reporting a supersession
/// that did not happen.
fn supersede(args: &UpdateArgs<'_>) -> Result<TripleEndOutcome, AmError> {
    if args.new_value.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "new_value is required for a discrete update",
        ));
    }
    if args.new_item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "new_item_id is required for a discrete update",
        ));
    }
    let bank = args.ctx.bank.as_str();
    let prior = args
        .store
        .get_memory_item(bank, args.target)?
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::ItemNotFound,
                format!("item `{}` not found in bank", args.target),
            )
        })?;
    if prior.epistemic_kind == EpistemicKind::Belief {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "belief targets evolve via belief_observe appends; update is for \
             item-addressed records",
        ));
    }
    let timestamp = clio_store::canonical_write_stamp(args.timestamp)?;
    let spec = CorrectionSpec {
        bank_id: bank.to_owned(),
        prior_item_id: args.target.to_owned(),
        new_item: replacement_item(&prior, args.new_value, args.new_item_id, &timestamp),
        reason: SUPERSEDE_REASON.to_owned(),
        actor: args.ctx.actor.as_str().to_owned(),
        timestamp,
    };
    let outcome = args.store.correct_item(&spec)?;
    if outcome.already_superseded {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "item `{}` is already superseded; update the active revision",
                args.target
            ),
        ));
    }
    let replacement_id = Some(outcome.new_item_id);
    args.triple_sink.emit(TripleEvent {
        operation: "invalidate".into(),
        bank: bank.into(),
        actor: args.ctx.actor.as_str().into(),
        timestamp: spec.timestamp,
        edge_or_item_id: args.target.into(),
        prior_ids: vec![args.target.to_owned()],
        replacement_id: replacement_id.clone(),
    });
    Ok(TripleEndOutcome {
        closed_ids: vec![args.target.to_owned()],
        already_ended: false,
        replacement_id,
    })
}

/// Build the replacement revision from the prior one plus the new value.
///
/// `new_value` always becomes the non-authoritative gist; when it parses as a
/// JSON object it also becomes the authoritative snapshot (the prior snapshot
/// is retained otherwise). Create-time admission score, provenance, taxonomy,
/// and context are carried over untouched: a mutator never re-scores.
fn replacement_item(
    prior: &MemoryItem,
    new_value: &str,
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
        context: prior.context.clone(),
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
/// Non-numeric or non-finite payload.
fn parse_numeric(raw: &str) -> Result<f64, AmError> {
    let value: f64 = raw.trim().parse().map_err(|_| {
        AmError::new(
            ErrorCode::InvalidType,
            format!("continuous observation must be a finite number, got `{raw}`"),
        )
    })?;
    if !value.is_finite() {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "continuous observation must be finite (NaN/Inf rejected)",
        ));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "update_fail_closed_tests.rs"]
mod update_fail_closed_tests;

#[cfg(test)]
#[path = "update_tests.rs"]
pub(crate) mod update_tests;
