// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the generic `update(item_id, new_value,
//! update_rule)` rule splitter and continuous mutator telemetry.
//!
//! ## Owns
//! - `update` routing: `discrete` → invalidation path, `continuous` → EMA engine
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

use crate::triple::TripleSink;
use clio_store::{ContinuousStore, Store};
use clio_types::{AmError, ContinuousState, ErrorCode, OpContext, TripleEndOutcome, UpdateRule};
use serde::{Deserialize, Serialize};

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
    /// attribute key. `update_rule=discrete`: the item id to invalidate.
    pub target: &'a str,
    /// New value; parsed as a finite float on the continuous path.
    ///
    /// Ignored on the discrete path: this facade only closes the target's
    /// bi-temporal interval. Admitting replacement content here would bypass
    /// the admission gates, so callers write the replacement through
    /// the normal gated create tools and link it via `invalidate` directly.
    pub new_value: &'a str,
    /// Which rule the caller wants to apply.
    pub update_rule: UpdateRule,
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
///   invalidation path (both axes closed, never EMA); `new_value` is not
///   admitted (the gated create tools write replacements).
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
        UpdateRule::Discrete => {
            // `args.new_value` is deliberately not admitted here. Writing
            // replacement content on this path would bypass the
            // admission gates that every long-term write must pass. The facade
            // only closes the item's bi-temporal interval; callers write the
            // replacement through the normal gated create tools and may pass its
            // id to `invalidate` directly for explicit replacement linkage.
            let out = crate::triple::invalidate(
                args.store,
                args.ctx,
                args.target,
                None,
                args.timestamp,
                Some(UpdateRule::Discrete),
                args.triple_sink,
            )?;
            Ok(UpdateOutcome::Discrete(out))
        }
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
#[path = "update_tests.rs"]
mod update_tests;
