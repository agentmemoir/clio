// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deterministic `hygiene_noise_score`
//! composition (appendix): weighted signals, graded credential scoring,
//! `noise_reasons[]`, and the suggested-action bands.
//!
//! ## Owns
//! - [`score_noise`]: `clamp_01` weighted sum + graded credential floor
//!   (kind base plus run length) and the weaker token-run proxy band
//! - Suggested action bands (`discard` ≥ 0.90 / credential, `archive`
//!   0.70–0.89, `flag` 0.40–0.69, none below)
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
    /// Real credential evidence ids (`named_credential_key`, `pem_private_key`,
    /// `bearer_token`, `known_secret_format`); empty means no credential.
    pub secret_kinds: Vec<&'static str>,
    /// Longest matched credential value length in bytes, `0` when none.
    pub secret_run_len: usize,
    /// Longest token-shaped run length in bytes: a weak proxy, never a hit.
    pub token_run_len: Option<usize>,
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
/// w_dup·dup + w_utility·(1−utility) + w_staleness·staleness)`, where `secret`
/// is the graded credential strength (strongest kind base plus run-length
/// bonus) or, with no credential evidence, the weaker token-run proxy strength.
/// That strength also floors the score: a real credential never lands below its
/// kind base, and a bare long token stays above `flag` without reaching the
/// `discard` band. Weights are renormalized so custom deployments stay on the
/// same scale; identical inputs and config always yield identical output.
#[must_use]
pub fn score_noise(input: &ScoreInput, weights: &HygieneWeights) -> Scored {
    let w = normalized(weights);
    let credential = !input.secret_kinds.is_empty();
    let strength = if credential {
        let base = input
            .secret_kinds
            .iter()
            .map(|kind| kind_base(kind))
            .fold(0.0, f64::max);
        (base + length_bonus(input.secret_run_len)).min(1.0)
    } else {
        input.token_run_len.map_or(0.0, |len| {
            0.42 + f64::from(u32::try_from(len.min(120)).unwrap_or(120)) / 120.0 * 0.15
        })
    };
    let mut raw = w.secret * strength
        + w.pattern
            * (f64::from(u32::try_from(input.pattern_kinds.len().min(2)).unwrap_or(2)) / 2.0)
        + w.dup * f64::from(input.near_duplicate)
        + w.utility * (1.0 - input.utility_proxy.clamp(0.0, 1.0))
        + w.staleness * input.staleness.clamp(0.0, 1.0);
    if !(raw.is_finite()) {
        raw = 0.0;
    }
    let score = raw.clamp(0.0, 1.0).max(strength);
    let mut reasons: Vec<&'static str> = Vec::new();
    if credential {
        reasons.push("secret_credential");
        let mut kinds = input.secret_kinds.clone();
        kinds.sort_unstable();
        kinds.dedup();
        reasons.extend(kinds);
    } else if input.token_run_len.is_some() {
        reasons.push("token_shaped_run");
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
        suggested_action: suggested_action(score, credential),
    }
}

/// Credential kind bases: the stronger the evidence, the higher the base.
const KIND_BASES: &[(&str, f64)] = &[
    ("pem_private_key", 0.98),
    ("known_secret_format", 0.95),
    ("named_credential_key", 0.93),
    ("bearer_token", 0.90),
];

/// Base score for one credential kind; an unknown kind takes the weakest base.
fn kind_base(kind: &str) -> f64 {
    KIND_BASES
        .iter()
        .find(|(id, _)| kind == *id)
        .map_or(0.90, |(_, base)| *base)
}

/// Length bonus: `0.0..=0.05`, full at a 64-byte matched run.
fn length_bonus(run_len: usize) -> f64 {
    f64::from(u32::try_from(run_len.min(64)).unwrap_or(64)) / 64.0 * 0.05
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
