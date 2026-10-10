// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for admission threshold and prior knobs in the ranking env.
//!
//! ## Owns
//! - Per-key `θ_admit` overrides and default (0.55)
//! - Per-key content-type prior overrides and defaults
//! - Recency `τ` validation and co-activation constant checks
//!
//! ## Does not own
//! - Running admission scoring or retrieval
//! - Weight renormalize-or-reject math (see `ranking`)
//!
//! ## Boundary
//!
//! Changes to admission knob schema and validation should normally require changes here.
//! Changes to weight normalization should not require changes here.
//!
//! Keep this module focused on admission threshold and prior knobs.

use crate::error::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::BuildHasher;

/// Default `θ_admit` per key when no override is set.
pub const DEFAULT_THETA_ADMIT: f64 = 0.55;

/// Default recency decay constant `τ` in days.
pub const DEFAULT_RECENCY_TAU_DAYS: f64 = 30.0;

/// Default recency decay constant `τ` in days, as a named function for the
/// test surface. The env is no longer deserialized as a whole.
#[must_use]
pub const fn default_recency_tau() -> f64 {
    DEFAULT_RECENCY_TAU_DAYS
}

/// Every key that may carry a threshold or prior override.
///
/// `belief` is the belief-writes create key — it is **not** a
/// semantic category or episodic type tag; it only tunes the create-path
/// threshold and prior for `belief_observe`.
pub const KNOWN_ADMISSION_KEYS: &[&str] = &[
    "task_spec",
    "schema",
    "tool_config",
    "output_constraint",
    "persona",
    "triple",
    "gist",
    "task",
    "failure",
    "temporal",
    "belief",
];

/// Default hub degree limit: nodes with more above-threshold effective edges are
/// flagged for `hub_distill` (association-policy source of truth).
pub const DEFAULT_HUB_DEGREE_LIMIT: u32 = 16;

/// Default graph-expansion hop cap.
pub const DEFAULT_MAX_HOPS: u32 = 2;

/// Serde default for `hub_degree_limit` when a patch omits the field.
#[must_use]
pub const fn default_hub_degree_limit() -> u32 {
    DEFAULT_HUB_DEGREE_LIMIT
}

/// Serde default for `max_hops` when a patch omits the field.
#[must_use]
pub const fn default_max_hops() -> u32 {
    DEFAULT_MAX_HOPS
}

/// Co-activation constants (`η`, half-life days, `w_min`, hub/expand bounds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoactivationConstants {
    /// Learning rate η (default 0.15).
    pub eta: f64,
    /// Half-life in days (default 30).
    pub half_life_days: f64,
    /// Prune threshold `w_min` (default 0.05).
    pub w_min: f64,
    /// Hub degree limit (default 16). Nodes above it are flagged for distillation.
    #[serde(default = "default_hub_degree_limit")]
    pub hub_degree_limit: u32,
    /// Hard `expand_graph` hop cap (default 2).
    #[serde(default = "default_max_hops")]
    pub max_hops: u32,
}

impl Default for CoactivationConstants {
    fn default() -> Self {
        Self {
            eta: 0.15,
            half_life_days: 30.0,
            w_min: 0.05,
            hub_degree_limit: DEFAULT_HUB_DEGREE_LIMIT,
            max_hops: DEFAULT_MAX_HOPS,
        }
    }
}

/// Range-check co-activation constants.
///
/// # Errors
/// Out-of-range `eta`, non-positive half-life, out-of-range `w_min`, or a
/// zero hub/expansion bound.
pub fn validate_coactivation(c: &CoactivationConstants) -> Result<(), AmError> {
    if !(0.0..=1.0).contains(&c.eta) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "coactivation.eta must be in [0, 1]",
        ));
    }
    if !(c.half_life_days.is_finite() && c.half_life_days > 0.0) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "coactivation.half_life_days must be finite and > 0",
        ));
    }
    if !(0.0..=1.0).contains(&c.w_min) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "coactivation.w_min must be in [0, 1]",
        ));
    }
    if c.hub_degree_limit == 0 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "coactivation.hub_degree_limit must be positive",
        ));
    }
    if c.max_hops == 0 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "coactivation.max_hops must be positive",
        ));
    }
    Ok(())
}

/// Default content-type prior for a known key, else `None`.
#[must_use]
pub fn default_type_prior(key: &str) -> Option<f64> {
    match key {
        "task_spec" | "failure" => Some(0.85),
        "schema" | "tool_config" | "triple" => Some(0.80),
        "output_constraint" | "task" => Some(0.75),
        "persona" => Some(0.90),
        "temporal" => Some(0.70),
        "gist" => Some(0.40),
        // Belief-writes row: the belief create prior matches the
        // historical fallback so the knob is tunable without behavior change.
        "belief" => Some(0.50),
        _ => None,
    }
}

/// Shared key/value validation for threshold and prior override maps.
///
/// Keys validate in sorted order so multi-error output is deterministic.
fn validate_override_map<S: BuildHasher>(
    map: &HashMap<String, f64, S>,
    what: &str,
) -> Result<(), AmError> {
    let mut pairs: Vec<(&String, &f64)> = map.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in pairs {
        let lower = key.to_ascii_lowercase();
        if lower.contains("bypass") {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!("{what} override `{key}` is forbidden (gates cannot be disabled)"),
            ));
        }
        if !KNOWN_ADMISSION_KEYS.contains(&key.as_str()) {
            return Err(AmError::new(
                ErrorCode::UnknownPath,
                format!(
                    "{what} override key must be a known category, episodic type, or belief key, got `{key}`"
                ),
            ));
        }
        if !(0.0..=1.0).contains(value) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!("{what} override for `{key}` must be in [0, 1]"),
            ));
        }
    }
    Ok(())
}

/// Validate per-key `θ_admit` overrides.
///
/// # Errors
/// Unknown keys, bypass keys, or values outside `[0, 1]`.
pub fn validate_theta_overrides<S: BuildHasher>(
    map: &HashMap<String, f64, S>,
) -> Result<(), AmError> {
    validate_override_map(map, "theta")
}

/// Validate per-key content-type prior overrides.
///
/// # Errors
/// Unknown keys, bypass keys, or values outside `[0, 1]`.
pub fn validate_type_prior_overrides<S: BuildHasher>(
    map: &HashMap<String, f64, S>,
) -> Result<(), AmError> {
    validate_override_map(map, "type_prior")
}

/// Validate the recency decay constant `τ` in days.
///
/// # Errors
/// Non-finite or non-positive `τ`.
pub fn validate_recency_tau(tau_days: f64) -> Result<(), AmError> {
    if !(tau_days.is_finite() && tau_days > 0.0) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "recency_tau_days must be finite and > 0",
        ));
    }
    Ok(())
}

/// Resolve the effective threshold: per-key override wins, else the global.
#[must_use]
pub fn threshold_for<S: BuildHasher>(
    overrides: &HashMap<String, f64, S>,
    global_theta: f64,
    key: &str,
) -> f64 {
    overrides.get(key).copied().unwrap_or(global_theta)
}

/// Resolve the effective content-type prior: override wins, else the default table.
#[must_use]
pub fn prior_for<S: BuildHasher>(overrides: &HashMap<String, f64, S>, key: &str) -> f64 {
    overrides
        .get(key)
        .copied()
        .unwrap_or_else(|| default_type_prior(key).unwrap_or(0.5))
}

#[cfg(test)]
#[path = "admission_knobs_tests.rs"]
mod admission_knobs_tests;
