// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for deterministic five-factor admission signals.
//!
//! ## Owns
//! - Utility, confidence, novelty, and type-prior signal math, plus the
//!   recency factor's missing-prior rule
//! - The `factor_unavailable` error when a signal backend is down
//!
//! ## Does not own
//! - The recency curve arithmetic: `recency_decay` delegates to
//!   `clio_types::decay::half_life_factor`
//! - Threshold policy or admit/reject decisions
//! - Durable writes or telemetry sinks
//!
//! ## Boundary
//!
//! Changes to factor formulas should normally require changes here.
//! Changes to thresholds or write wiring should not require changes here.
//!
//! Keep this module focused on deterministic factor signals.

use clio_types::{AmError, ErrorCode, SourceType, half_life_factor};
use std::collections::BTreeSet;

/// Clamp a signal into `[0, 1]`; non-finite values become `0.0`.
#[must_use]
pub fn clamp01(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

/// Structured error when a required signal backend is unavailable.
///
/// Callers must not write and must not emit a reject event for this error.
#[must_use]
pub fn factor_unavailable(what: &str) -> AmError {
    AmError::new(
        ErrorCode::FactorUnavailable,
        format!("factor_unavailable: {what}"),
    )
}

/// Future utility from explicit markers: preference/decision/constraint/task
/// markers score high, one-off chatter scores low, everything else is neutral.
#[must_use]
pub fn utility_score(text: &str) -> f64 {
    const HIGH: &[&str] = &[
        "prefer",
        "decision",
        "decided",
        "constraint",
        "must",
        "require",
        "task",
        "todo",
        "goal",
        "policy",
    ];
    const CHATTER: &[&str] = &[
        "hi", "hello", "hey", "thanks", "thank", "ok", "okay", "bye", "yes", "no",
    ];
    let lower = text.to_ascii_lowercase();
    if HIGH.iter().any(|m| lower.contains(m)) {
        return 0.9;
    }
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if text.trim().len() < 24 && words.iter().any(|w| CHATTER.contains(w)) {
        return 0.15;
    }
    0.5
}

/// Factual confidence from provenance: direct statements score above inferred
/// ones, and speculative markers cap the score low.
#[must_use]
pub fn confidence_score(source_type: Option<SourceType>, text: &str) -> f64 {
    const SPECULATIVE: &[&str] = &[
        "maybe", "perhaps", "possibly", "might", "guess", "probably", "unsure", "not sure",
    ];
    let base: f64 = match source_type {
        Some(SourceType::UserStated) => 0.9,
        Some(SourceType::ThirdParty) => 0.7,
        Some(SourceType::AgentInferred) => 0.6,
        None => 0.5,
    };
    let lower = text.to_ascii_lowercase();
    if SPECULATIVE.iter().any(|m| lower.contains(m)) {
        return base.min(0.3);
    }
    base
}

/// Lowercase alphanumeric word set for the novelty proxy.
fn token_set(text: &str) -> BTreeSet<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Jaccard similarity of two token sets.
///
/// Only called with a non-empty candidate (empty candidates return early in
/// [`novelty_jaccard`]), so the union is never empty here.
fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    // Token counts are bounded by input text length, far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let (inter, union) = (inter as f64, union as f64);
    inter / union
}

/// Semantic novelty proxy: `1 - max_jaccard` over neighbor texts.
///
/// Empty candidates score `0.0`; with no neighbors the score is `1.0`.
/// Exact token-set duplicates score `0.0`.
#[must_use]
pub fn novelty_jaccard(text: &str, neighbors: &[String]) -> f64 {
    let candidate = token_set(text);
    if candidate.is_empty() {
        return 0.0;
    }
    if neighbors.is_empty() {
        return 1.0;
    }
    let best = neighbors
        .iter()
        .map(|n| jaccard(&candidate, &token_set(n)))
        .fold(0.0_f64, f64::max);
    clamp01(1.0 - best)
}

/// Lowercase, punctuation-free, whitespace-collapsed form used to decide
/// whether two texts are the *same* sentence.
///
/// Word order is preserved on purpose. A token-set comparison would call
/// "Page 1 Item 2" and "Page 2 Item 1" identical — and likewise "Tom walks with
/// Alice" and "Alice walks with Tom" — which are different facts wearing the
/// same words. Novelty may ignore order, because it is a similarity proxy; the
/// duplicate rejection may not, because it refuses to store a real fact.
fn normalize_exact(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        } else {
            pending_space = !out.is_empty();
        }
    }
    out
}

/// True when the candidate says the same thing as some stored neighbor.
///
/// Two texts match when their [`normalize_exact`] forms are equal, so case,
/// punctuation, and spacing are ignored but word order is not. This is
/// deliberately separate from [`novelty_jaccard`], which also returns `0.0` for
/// an empty candidate and which scores near-duplicates rather than identifying
/// them; the two signals must not be read interchangeably.
#[must_use]
pub fn is_exact_duplicate(text: &str, neighbors: &[String]) -> bool {
    let normalized = normalize_exact(text);
    if normalized.is_empty() {
        return false;
    }
    neighbors
        .iter()
        .any(|neighbor| normalize_exact(neighbor) == normalized)
}

/// Temporal recency: `exp(-ln2 * age_days / tau_days)`, so `tau_days` acts as
/// the half-life and the factor halves once per tau; no prior scores `1.0`.
///
/// The curve is deliberately a half-life decay, not the plain exponential
/// `exp(-age / tau)` this factor once used: at one tau the score is now `0.5`
/// rather than `0.368`. A non-positive or non-finite tau, and a NaN age, still
/// fail soft at `0.0` via [`half_life_factor`].
#[must_use]
pub fn recency_decay(prior_age_days: Option<f64>, tau_days: f64) -> f64 {
    let Some(age) = prior_age_days else {
        return 1.0;
    };
    half_life_factor(age, tau_days)
}

/// Weighted admission score from the five factors in `w1…w5` order.
#[must_use]
pub fn weighted_score(weights: [f64; 5], factors: [f64; 5]) -> f64 {
    clamp01(
        weights[0] * factors[0]
            + weights[1] * factors[1]
            + weights[2] * factors[2]
            + weights[3] * factors[3]
            + weights[4] * factors[4],
    )
}

#[cfg(test)]
#[path = "factors_determinism_tests.rs"]
mod factors_determinism_tests;
#[cfg(test)]
#[path = "factors_tests.rs"]
mod factors_tests;
