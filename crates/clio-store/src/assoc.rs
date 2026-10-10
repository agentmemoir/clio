// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure `association_weight_policy` math for
//! `assoc_kind=coactivation` edges (saturating growth, lazy decay, prune
//! decision) and the canonical pair/edge-id helpers.
//!
//! ## Owns
//! - `AssocPolicy` knobs plus saturating growth, lazy effective weight, prune
//! - Canonical pair ordering and deterministic edge ids
//!
//! ## Does not own
//! - Persistence (see the `Store` trait and backend modules)
//! - The continuous-attribute EMA engine (`clio-write`); this policy never uses it
//! - The shared half-life factor arithmetic (`clio_types::decay`); this module
//!   only multiplies the factor by the edge weight and clamps the product
//!
//! ## Boundary
//!
//! Changes to the association weight formula should normally require changes
//! here. Changes to backend storage SQL should not require changes here.

use clio_config::ranking::CoactivationConstants;
use clio_types::half_life_factor;

/// Separator unlikely to appear in opaque item ids, used for deterministic ids.
const ID_SEP: char = '\u{1f}';

/// Co-activation policy knobs resolved from the ranking env.
#[derive(Debug, Clone, PartialEq)]
pub struct AssocPolicy {
    /// Saturating growth learning rate `η`.
    pub eta: f64,
    /// Half-life in days for lazy decay.
    pub half_life_days: f64,
    /// Prune threshold `w_min` (effective weight below it is removed).
    pub w_min: f64,
    /// Hub degree limit above which a node is flagged for `hub_distill`.
    pub hub_degree_limit: u32,
    /// Hard hop cap for graph expansion.
    pub max_hops: u32,
    /// Max expanded neighbors merged into a candidate set.
    pub expand_neighbor_cap: usize,
    /// Max seed nodes expanded per retrieve.
    pub expand_seed_cap: usize,
}

impl Default for AssocPolicy {
    fn default() -> Self {
        Self::from_coactivation(&CoactivationConstants::default())
    }
}

impl AssocPolicy {
    /// Resolve policy knobs from the ranking-env co-activation constants.
    #[must_use]
    pub fn from_coactivation(c: &CoactivationConstants) -> Self {
        Self {
            eta: c.eta,
            half_life_days: c.half_life_days,
            w_min: c.w_min,
            hub_degree_limit: c.hub_degree_limit,
            max_hops: c.max_hops,
            expand_neighbor_cap: 64,
            expand_seed_cap: 32,
        }
    }
}

/// Canonical unordered pair: `(min, max)` over the two ids.
#[must_use]
pub fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

/// Deterministic id for the single co-activation edge of an unordered pair.
#[must_use]
pub fn coactivation_id(bank: &str, a: &str, b: &str) -> String {
    let (lo, hi) = canonical_pair(a, b);
    format!("coact{ID_SEP}{bank}{ID_SEP}{lo}{ID_SEP}{hi}")
}

/// Deterministic id for an explicit edge `(pair, relationship)`.
#[must_use]
pub fn explicit_id(bank: &str, a: &str, b: &str, relationship: &str) -> String {
    let (lo, hi) = canonical_pair(a, b);
    format!("expl{ID_SEP}{bank}{ID_SEP}{lo}{ID_SEP}{hi}{ID_SEP}{relationship}")
}

/// One saturating growth step `w ← w + η·(1 − w)`, clamped to `[0, 1]`.
///
/// This is **not** the continuous EMA formula `α·x + (1 − α)·prev`.
#[must_use]
pub fn grow(weight: f64, eta: f64) -> f64 {
    (weight + eta * (1.0 - weight)).clamp(0.0, 1.0)
}

/// Parse an epoch-millis text stamp (the store's portable clock form).
#[must_use]
pub fn parse_millis(raw: &str) -> Option<i64> {
    raw.parse::<i64>().ok()
}

/// Lazy effective weight `w·exp(−λ·Δt)` for a coactivation edge.
///
/// `last_reinforced_at` is the edge's raw timestamp text. When it is not an
/// epoch-millis stamp (legacy rows), the raw weight is returned unchanged so a
/// malformed stamp never silently decays an edge.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn effective_weight(
    weight: f64,
    last_reinforced_at: &str,
    now_millis: i64,
    half_life_days: f64,
) -> f64 {
    let Some(last) = parse_millis(last_reinforced_at) else {
        return weight;
    };
    let elapsed_millis = (now_millis - last).max(0);
    let elapsed_days = elapsed_millis as f64 / 86_400_000.0;
    (weight * half_life_factor(elapsed_days, half_life_days)).clamp(0.0, 1.0)
}

/// Prune decision: true when the effective weight has fallen below `w_min`.
#[must_use]
pub fn should_prune(effective: f64, w_min: f64) -> bool {
    effective < w_min
}

#[cfg(test)]
#[path = "assoc_policy_tests.rs"]
mod assoc_policy_tests;

#[cfg(test)]
#[path = "assoc_weight_tests.rs"]
mod assoc_weight_tests;

#[cfg(test)]
#[path = "assoc_edge_tests.rs"]
mod assoc_edge_tests;
