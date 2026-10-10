// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for continuous-attribute schema types and the
//! pure EMA + trend update math (the only shared continuous engine).
//!
//! ## Owns
//! - `AttributeSchema` (`update_rule` declaration, α/N, bounds) and its validation
//! - `ContinuousState` / `ContinuousPoint` shapes and `apply_observation` formulas
//! - Cold start, bounds reject/clamp, and trend-window edge cases
//!
//! ## Does not own
//! - Persistence drivers, concurrency control, or the `update` rule splitter
//! - `epistemic_kind` (`fact`|`belief`) — a separate enum on a separate column
//!
//! ## Boundary
//!
//! Changes to `update_rule` declarations or formulas should normally require changes here.
//! Changes to SQL layout or mutator wiring should not require changes here.
//!
//! Keep this module focused on continuous-attribute declarations and the shared formula engine.

use crate::error::{AmError, ErrorCode};
use crate::triple::UpdateRule;
use serde::{Deserialize, Serialize};

/// Default per-attribute smoothing weight (appendix suggestion; schema may override).
pub const DEFAULT_ALPHA: f64 = 0.3;
/// Default trend window `N` (mean over the N most recent EMA states).
pub const DEFAULT_TREND_WINDOW: u32 = 10;

/// Optional observation range `[lo, hi]` for a continuous attribute.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// Lower bound (inclusive).
    pub lo: f64,
    /// Upper bound (inclusive).
    pub hi: f64,
}

/// Effective engine knobs resolved from a schema (defaults when unset).
#[derive(Debug, Clone, PartialEq)]
pub struct ContinuousEngineConfig {
    /// EMA weight α ∈ (0, 1].
    pub alpha: f64,
    /// Trend window N (mean of the N most recent states).
    pub trend_window: u32,
    /// Optional declared value bounds.
    pub bounds: Option<Bounds>,
    /// Clamp out-of-range observations instead of rejecting (schema opt-in).
    pub clamp: bool,
}

impl ContinuousEngineConfig {
    /// Config from documented defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            alpha: DEFAULT_ALPHA,
            trend_window: DEFAULT_TREND_WINDOW,
            bounds: None,
            clamp: false,
        }
    }

    /// Resolve knobs from an attribute schema (declaration wins over defaults).
    #[must_use]
    pub fn from_schema(schema: &AttributeSchema) -> Self {
        Self {
            alpha: schema.alpha.unwrap_or(DEFAULT_ALPHA),
            trend_window: schema.trend_window.unwrap_or(DEFAULT_TREND_WINDOW),
            bounds: match (schema.bounds_lo, schema.bounds_hi) {
                (Some(lo), Some(hi)) => Some(Bounds { lo, hi }),
                _ => None,
            },
            clamp: schema.clamp_values,
        }
    }

    /// Validate knobs (α ∈ (0,1], N ≥ 1, sane bounds).
    ///
    /// # Errors
    /// `OutOfRange` when any knob violates its constraint.
    pub fn validate(&self) -> Result<(), AmError> {
        if !(self.alpha.is_finite() && self.alpha > 0.0 && self.alpha <= 1.0) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "alpha must be a finite value in (0, 1]",
            ));
        }
        if self.trend_window < 1 {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "trend_window must be >= 1",
            ));
        }
        if let Some(b) = &self.bounds
            && !(b.lo.is_finite() && b.hi.is_finite() && b.lo < b.hi)
        {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "bounds must be finite with lo < hi",
            ));
        }
        Ok(())
    }
}

/// Definition-time declaration of one updatable attribute.
///
/// `update_rule` is its own declaration — never stored in or read from the item
/// `epistemic_kind` (`fact`|`belief`) column (vocabulary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeSchema {
    /// Bank isolation key.
    pub bank_id: String,
    /// Attribute key (e.g. `affinity`); unique per bank.
    pub attr_key: String,
    /// The declared rule: `discrete` | `continuous`.
    pub update_rule: UpdateRule,
    /// Optional per-attribute α (default [`DEFAULT_ALPHA`]).
    pub alpha: Option<f64>,
    /// Optional per-attribute trend window N (default [`DEFAULT_TREND_WINDOW`]).
    pub trend_window: Option<u32>,
    /// Optional lower bound.
    pub bounds_lo: Option<f64>,
    /// Optional upper bound.
    pub bounds_hi: Option<f64>,
    /// Clamp out-of-range observations instead of rejecting them.
    pub clamp_values: bool,
    /// Created timestamp.
    pub created_at: String,
    /// Updated timestamp.
    pub updated_at: Option<String>,
}

impl AttributeSchema {
    /// Structural validation at registration time.
    ///
    /// # Errors
    /// Empty ids, out-of-range knobs, or bounds/clamp inconsistencies.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.bank_id.is_empty() || self.attr_key.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and attr_key are required",
            ));
        }
        if self.created_at.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "created_at is required",
            ));
        }
        if let Some(a) = self.alpha
            && !(a.is_finite() && (0.0..=1.0).contains(&a) && a > 0.0)
        {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "alpha must be a finite value in (0, 1]",
            ));
        }
        if self.trend_window.is_some_and(|n| n < 1) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "trend_window must be >= 1",
            ));
        }
        match (self.bounds_lo, self.bounds_hi) {
            (None, None) => {}
            (Some(lo), Some(hi)) => {
                if !(lo.is_finite() && hi.is_finite() && lo < hi) {
                    return Err(AmError::new(
                        ErrorCode::OutOfRange,
                        "bounds must be finite with lo < hi",
                    ));
                }
            }
            _ => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "bounds_lo and bounds_hi must be set together",
                ));
            }
        }
        if self.clamp_values && self.bounds_lo.is_none() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "clamp_values=true requires declared bounds",
            ));
        }
        Ok(())
    }
}

/// Current continuous state for one attribute key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousState {
    /// Current EMA state `s_t`.
    pub state: f64,
    /// Slower trend baseline (mean of the last N states).
    pub trend: f64,
    /// Observation timestamp of the applied observation.
    pub as_of: String,
    /// Last min(N, t+1) EMA states, oldest → newest.
    pub window: Vec<f64>,
}

/// One retained observation point for temporal trajectories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousPoint {
    /// Observation timestamp.
    pub as_of: String,
    /// EMA state after the observation.
    pub state: f64,
    /// Trend after the observation.
    pub trend: f64,
    /// Optional bounded descriptive source context. A point carries the
    /// key's stored context only when this point's own `as_of` is at or after
    /// the timestamp the context became effective (the preference row's
    /// `context_as_of`, falling back to the row's `as_of` on a legacy row);
    /// earlier points carry `None`. The store keeps one context per key, so a
    /// point carries the context effective from its introduction onward, not a
    /// per-observation context history. Omitted when the key has none or the
    /// sealed value is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Apply one continuous observation.
///
/// `s_t = α·x + (1−α)·s_{t−1}`; cold start `s_0 = x_0`, `trend_0 = x_0`;
/// `trend_t = mean(last N states)` (mean of all available when fewer than N).
///
/// # Errors
/// Non-finite value, out-of-bounds value (when not clamping), or invalid config.
pub fn apply_observation(
    config: &ContinuousEngineConfig,
    prior_window: &[f64],
    value: f64,
    as_of: &str,
) -> Result<ContinuousState, AmError> {
    config.validate()?;
    if !value.is_finite() {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "observation must be a finite float (NaN/Inf rejected)",
        ));
    }
    let value = match config.bounds {
        Some(b) if value < b.lo || value > b.hi => {
            if config.clamp {
                value.clamp(b.lo, b.hi)
            } else {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("observation {value} outside declared bounds"),
                ));
            }
        }
        _ => value,
    };
    let prior_state = prior_window.last().copied();
    let state = prior_state.map_or(value, |s| config.alpha * value + (1.0 - config.alpha) * s);
    let keep = config.trend_window as usize;
    let mut window: Vec<f64> = prior_window.to_vec();
    window.push(state);
    if window.len() > keep {
        window.drain(..window.len() - keep);
    }
    let trend = mean_last_n(&window, keep);
    Ok(ContinuousState {
        state,
        trend,
        as_of: as_of.to_owned(),
        window,
    })
}

/// Arithmetic mean of the last `n` states (all available when shorter).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn mean_last_n(window: &[f64], n: usize) -> f64 {
    let start = window.len().saturating_sub(n);
    let slice = &window[start..];
    let sum: f64 = slice.iter().sum();
    sum / (slice.len() as f64)
}

#[cfg(test)]
#[path = "attribute_tests.rs"]
mod attribute_tests;
