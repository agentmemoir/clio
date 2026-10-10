// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deterministic `hygiene_noise_score`
//! composition (appendix): weighted signals, the secret-score floor,
//! `noise_reasons[]`, and the suggested-action bands.
//!
//! ## Owns
//! - [`score_noise`]: `clamp_01` weighted sum + secret override (≥ 0.90)
//! - Suggested action bands (`discard` ≥ 0.90 / secret, `archive` 0.70–0.89,
//!   `flag` 0.40–0.69, none below)
//!
//! ## Does not own
//! - Signal detection heuristics (see `detect`)
//! - Scanning, ranking, or mutation (`audit` / `clean`)
//!
//! ## Boundary
//!
//! Changes to the formula, weights application, or action bands should normally
//! require changes here. Changes to detectors should not require changes here.
//!
//! Keep this module focused on noise score composition.

use clio_config::ranking::HygieneWeights;
use serde::Serialize;

/// Scored candidate: noise score, explainable reasons, and suggested action.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scored {
    /// `hygiene_noise_score` in `0.0..=1.0`.
    pub score: f64,
    /// Explainable factor ids, ordered deterministically.
    pub reasons: Vec<&'static str>,
    /// Suggested action band (`flag` | `archive` | `discard`), or `None`
    /// below 0.40.
    pub suggested_action: Option<&'static str>,
}

/// Inputs for one noise-score computation.
#[derive(Debug, Clone)]
pub struct ScoreInput {
    /// Detected secret/pattern signals.
    pub secret_hit: bool,
    /// Operational pattern class ids (empty = none).
    pub pattern_kinds: Vec<&'static str>,
    /// Near-duplicate of a higher-utility retained item.
    pub near_duplicate: bool,
    /// Coarse utility proxy in `0.0..=1.0` (admission score or neutral 0.5).
    pub utility_proxy: f64,
    /// Age staleness in `0.0..=1.0` (age / configured half-life, clamped).
    pub staleness: f64,
}

/// Compose the deterministic noise score for one candidate.
///
/// `hygiene_noise_score = clamp_01(w_secret·secret + w_pattern·pattern +
/// w_dup·dup + w_utility·(1−utility) + w_staleness·staleness)`; a secret hit
/// forces `max(score, 0.90)` with the `secret_credential` reason.
/// Weights are renormalized so custom deployments stay on the same scale;
/// identical inputs and config always yield identical output.
#[must_use]
pub fn score_noise(input: &ScoreInput, weights: &HygieneWeights) -> Scored {
    let w = normalized(weights);
    let mut raw = w.secret * f64::from(input.secret_hit)
        + w.pattern
            * (f64::from(u32::try_from(input.pattern_kinds.len().min(2)).unwrap_or(2)) / 2.0)
        + w.dup * f64::from(input.near_duplicate)
        + w.utility * (1.0 - input.utility_proxy.clamp(0.0, 1.0))
        + w.staleness * input.staleness.clamp(0.0, 1.0);
    if !(raw.is_finite()) {
        raw = 0.0;
    }
    let mut score = raw.clamp(0.0, 1.0);
    let mut reasons: Vec<&'static str> = Vec::new();
    if input.secret_hit {
        reasons.push("secret_credential");
        score = score.max(0.90);
    }
    reasons.extend_from_slice(&input.pattern_kinds);
    if input.near_duplicate {
        reasons.push("near_duplicate");
    }
    if input.utility_proxy < 0.5 {
        reasons.push("low_utility");
    }
    if input.staleness >= 0.5 {
        reasons.push("stale");
    }
    Scored {
        score,
        reasons,
        suggested_action: suggested_action(score, input.secret_hit),
    }
}

/// Appendix action band.
#[must_use]
pub fn suggested_action(score: f64, secret_hit: bool) -> Option<&'static str> {
    if secret_hit || score >= 0.90 {
        Some("discard")
    } else if score >= 0.70 {
        Some("archive")
    } else if score >= 0.40 {
        Some("flag")
    } else {
        None
    }
}

/// Renormalized weight vector (deployment overrides may not sum to 1).
fn normalized(weights: &HygieneWeights) -> HygieneWeights {
    let sum: f64 = [
        weights.secret,
        weights.pattern,
        weights.dup,
        weights.utility,
        weights.staleness,
    ]
    .iter()
    .sum();
    if !(sum.is_finite()) || sum <= 0.0 {
        return HygieneWeights::default();
    }
    let f = |v: f64| v / sum;
    HygieneWeights {
        secret: f(weights.secret),
        pattern: f(weights.pattern),
        dup: f(weights.dup),
        utility: f(weights.utility),
        staleness: f(weights.staleness),
    }
}

#[cfg(test)]
#[path = "scoring_tests.rs"]
mod scoring_tests;
