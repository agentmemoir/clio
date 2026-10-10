// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the thin continuous-observe adapter stub that
//! later persona companion work must call into.
//!
//! ## Owns
//! - `observe_continuous` adapter contract: telemetry + single engine call target
//!
//! ## Does not own
//! - EMA math (lives in `clio-types`; persona MUST NOT reimplement it)
//! - The full persona object, token budget, or `persona_*` tool surface
//!
//! ## Boundary
//!
//! Changes to the adapter contract shape should normally require changes here.
//! Changes to formula math or store drivers should not require changes here.
//!
//! Keep this module focused on the thin continuous-observe adapter contract.

use clio_store::ContinuousStore;
use clio_types::{AmError, ContinuousState, OpContext};

/// Observe one scalar preference value for a declared continuous attribute.
///
/// Adapter contract (downstream persona work MUST respect this):
/// - Persona preference observation calls THIS function (or the shared `update`
///   facade), which routes to the store's continuous RMW and the engine.
/// - Persona code MUST NOT keep a second EMA copy; there is exactly one
///   implementation (`clio-types::apply_observation` behind the store method).
/// - The attribute key must already be declared `continuous` in the bank
///   (registration happens via `put_attribute_schema`).
///
/// # Errors
/// Undeclared/mismatched rule, invalid value, or store failures.
pub fn observe_continuous(
    store: &dyn ContinuousStore,
    ctx: &OpContext,
    attr_key: &str,
    observed_value: f64,
    as_of: &str,
    sink: &dyn crate::update::ContinuousSink,
) -> Result<ContinuousState, AmError> {
    let state = store.observe_continuous(
        ctx.bank.as_str(),
        attr_key,
        observed_value,
        as_of,
        ctx.actor.as_str(),
    )?;
    sink.emit(crate::update::ContinuousEvent {
        operation: "observe_continuous".into(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        attr_key: attr_key.into(),
        timestamp: state.as_of.clone(),
        state: state.state,
        trend: state.trend,
    });
    Ok(state)
}

#[cfg(test)]
#[path = "persona_tests.rs"]
mod persona_tests;
