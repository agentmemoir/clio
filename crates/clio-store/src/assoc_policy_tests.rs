// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure association weight-policy fixtures
//! (numeric oracles): growth, decay, prune, and id canonicalization.
//!
//! ## Owns
//! - Unit fixtures for `assoc::{grow, effective_weight, should_prune, *_id}`
//!
//! ## Does not own
//! - Persistence or traversal behavior (covered by other `*_tests` modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_config::ranking::CoactivationConstants;

const DAY_MILLIS: i64 = 86_400_000;

#[test]
fn canonical_pair_and_ids_are_order_invariant() {
    assert_eq!(canonical_pair("b", "a"), ("a".to_owned(), "b".to_owned()));
    assert_eq!(canonical_pair("a", "b"), ("a".to_owned(), "b".to_owned()));
    assert_eq!(
        coactivation_id("bank", "b", "a"),
        coactivation_id("bank", "a", "b")
    );
    assert_eq!(
        explicit_id("bank", "b", "a", "depends_on"),
        explicit_id("bank", "a", "b", "depends_on")
    );
    // Different relationships yield distinct explicit ids (multi-relationship).
    assert_ne!(
        explicit_id("bank", "a", "b", "depends_on"),
        explicit_id("bank", "a", "b", "caused_by")
    );
    // Coactivation and explicit ids never collide.
    assert_ne!(
        coactivation_id("bank", "a", "b"),
        explicit_id("bank", "a", "b", "x")
    );
}

#[test]
fn grow_matches_appendix_step_table() {
    let mut w = 0.0;
    for expected in [0.15, 0.2775, 0.385_875] {
        w = grow(w, 0.15);
        assert!((w - expected).abs() < 1e-12, "got {w}, want {expected}");
    }
    assert!(grow(1.0, 0.15) <= 1.0);
    assert!((grow(0.5, 0.0) - 0.5).abs() < 1e-12);
    assert!((grow(0.0, 1.0) - 1.0).abs() < 1e-12);
    for w in [0.0, 0.2, 0.5, 0.9, 1.0] {
        let g = grow(w, 0.15);
        assert!((0.0..=1.0).contains(&g), "grow escaped [0,1]: {g}");
    }
}

#[test]
fn grow_is_not_the_ema_engine() {
    // Continuous EMA (`α·x + (1−α)·prev`) with observation x=0 diverges from
    // the saturating co-activation step, which has no observation term.
    let prev = 0.5;
    let alpha = 0.15;
    let ema = alpha * 0.0 + (1.0 - alpha) * prev; // 0.425
    let saturating = grow(prev, alpha); // 0.575
    assert!(
        (saturating - ema).abs() > 1e-6,
        "sat {saturating} ema {ema}"
    );
}

#[test]
fn saturating_growth_stays_bounded_property() {
    // Property: ∀ w∈[0,1], η∈(0,1]: grow(w,η) ∈ [0,1].
    for wi in 0..=100 {
        let w = f64::from(wi) / 100.0;
        for ei in 1..=100 {
            let eta = f64::from(ei) / 100.0;
            let g = grow(w, eta);
            assert!((0.0..=1.0).contains(&g), "w={w} eta={eta} -> {g}");
            assert!(g >= w - 1e-12, "growth must not decrease: {w} -> {g}");
        }
    }
}

#[test]
fn effective_weight_half_life_and_edge_cases() {
    let now = 1_000 * DAY_MILLIS;
    // Δt = 30d, half_life = 30d, w = 1 → ~0.5.
    let w_eff = effective_weight(1.0, &(now - 30 * DAY_MILLIS).to_string(), now, 30.0);
    assert!((w_eff - 0.5).abs() < 1e-9, "got {w_eff}");
    // Δt = 0 → unchanged.
    assert!((effective_weight(0.8, &now.to_string(), now, 30.0) - 0.8).abs() < 1e-12);
    // Future stamp (clock skew) never grows the weight.
    assert!(
        (effective_weight(0.8, &(now + DAY_MILLIS).to_string(), now, 30.0) - 0.8).abs() < 1e-12
    );
    // Unparseable legacy stamp → raw weight, no silent decay.
    assert!((effective_weight(0.42, "2024-01-01T00:00:00Z", now, 30.0) - 0.42).abs() < 1e-12);
    assert_eq!(parse_millis("12345"), Some(12_345));
    assert_eq!(parse_millis("not-a-number"), None);
}

#[test]
fn effective_weight_matches_edge_decay_table_row() {
    let half_life_days = 30.0;
    let now = 500 * DAY_MILLIS;
    // Pinned from the half-life curve exp(-ln2 · Δt / 30), computed from the
    // formula rather than from the implementation: the edge-decay table row.
    let row: [(i64, f64); 4] = [
        (7, 0.850_667),
        (30, 0.500_000),
        (90, 0.125_000),
        (365, 0.000_218),
    ];
    for (elapsed_days, expected) in row {
        let last = (now - elapsed_days * DAY_MILLIS).to_string();
        let got = effective_weight(1.0, &last, now, half_life_days);
        assert!(
            (got - expected).abs() < 1e-6,
            "Δt={elapsed_days}d: got {got}, want {expected}"
        );
    }
}

#[test]
fn prune_and_policy_resolution() {
    assert!(should_prune(0.049, 0.05));
    assert!(!should_prune(0.05, 0.05));
    let c = CoactivationConstants {
        eta: 0.2,
        half_life_days: 10.0,
        w_min: 0.1,
        hub_degree_limit: 4,
        max_hops: 3,
    };
    let p = AssocPolicy::from_coactivation(&c);
    assert!((p.eta - 0.2).abs() < 1e-12);
    assert!((p.w_min - 0.1).abs() < 1e-12);
    assert_eq!(p.hub_degree_limit, 4);
    assert_eq!(p.max_hops, 3);
    assert_eq!(AssocPolicy::default().hub_degree_limit, 16);
}
