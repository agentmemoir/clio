// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the association-policy unit fixtures that the
//! sibling numeric-oracle module does not already assert: fixed policy caps,
//! half-life constant mapping, observable id shape, clamp boundaries, and
//! parse/decay edge values.
//!
//! ## Owns
//! - Unit fixtures for `AssocPolicy::{default, from_coactivation}` and
//!   `assoc::{canonical_pair, coactivation_id, explicit_id, grow, parse_millis,
//!   effective_weight, should_prune}` beyond the sibling numeric oracles
//!
//! ## Does not own
//! - Persistence or traversal behavior (covered by other `*_tests` modules)
//! - The numeric-oracle fixtures in the sibling policy test module
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_config::ranking::CoactivationConstants;

const DAY_MILLIS: i64 = 86_400_000;

#[test]
fn default_policy_equals_from_coactivation_with_caps() {
    let c = CoactivationConstants::default();
    assert_eq!(AssocPolicy::default(), AssocPolicy::from_coactivation(&c));
    let p = AssocPolicy::default();
    // Caps are fixed by the policy, not by the ranking env.
    assert_eq!(p.expand_neighbor_cap, 64);
    assert_eq!(p.expand_seed_cap, 32);
}

#[test]
fn from_coactivation_maps_half_life_and_fixed_caps() {
    let c = CoactivationConstants {
        half_life_days: 7.5,
        ..CoactivationConstants::default()
    };
    let p = AssocPolicy::from_coactivation(&c);
    assert!((p.half_life_days - 7.5).abs() < 1e-12);
    // Caps stay fixed even when the constants differ from the defaults.
    assert_eq!(p.expand_neighbor_cap, 64);
    assert_eq!(p.expand_seed_cap, 32);
}

#[test]
fn canonical_pair_handles_equal_ids() {
    // Equal ids are their own canonical pair; the two strict orderings are
    // pinned by the sibling numeric-oracle module.
    assert_eq!(
        canonical_pair("same", "same"),
        ("same".into(), "same".into())
    );
}

#[test]
fn ids_embed_prefix_bank_and_pair_in_canonical_order() {
    let a = coactivation_id("bank", "zz", "aa");
    assert!(a.starts_with("coact"), "unexpected coactivation id: {a}");
    assert!(a.contains("bank") && a.contains("aa") && a.contains("zz"));
    assert!(a.find("aa") < a.find("zz"), "pair not canonical: {a}");
    let e = explicit_id("bank", "zz", "aa", "related_to");
    assert!(e.starts_with("expl") && e.contains("related_to"));
    assert!(e.find("aa") < e.find("zz"), "pair not canonical: {e}");
    // Distinct banks never collide.
    assert_ne!(
        coactivation_id("bank", "a", "b"),
        coactivation_id("other", "a", "b")
    );
}

#[test]
fn grow_clamps_and_is_idempotent_at_one() {
    // A weight already at 1.0 stays at 1.0, whatever eta does.
    assert!((grow(1.0, 100.0) - 1.0).abs() < 1e-12);
    assert!((grow(1.0, 0.15) - 1.0).abs() < 1e-12);
    // A large positive eta saturates a mid weight at 1.0.
    assert!((grow(0.5, 50.0) - 1.0).abs() < 1e-12);
    // A negative eta pushes below 0 and clamps to 0.0.
    assert!((grow(0.05, -0.9) - 0.0).abs() < 1e-12);
    // Applying the step again at the ceiling changes nothing.
    let once = grow(1.0, 0.15);
    assert!((grow(once, 0.15) - once).abs() < 1e-12);
}

#[test]
fn parse_millis_accepts_numeric_and_rejects_other_text() {
    // The sibling oracle covers a generic `Some`/`None`; these are the
    // boundary values it does not.
    assert_eq!(parse_millis("0"), Some(0));
    assert_eq!(parse_millis("-86400000"), Some(-86_400_000));
    assert_eq!(parse_millis(""), None);
}

#[test]
fn effective_weight_legacy_stamp_returns_raw_weight() {
    let now = 1_000 * DAY_MILLIS;
    // Unparseable legacy stamp: fail-soft, no silent decay and no clamping —
    // even an out-of-range weight passes through untouched.
    assert!((effective_weight(1.5, "", now, 30.0) - 1.5).abs() < 1e-12);
}

/// Current non-millis writer: the MCP batch preflight stages `last_updated_at`
/// from `resolve_timestamp`, which yields an ISO UTC stamp. That live shape,
/// not only junk text, must keep the raw weight untouched.
#[test]
fn effective_weight_iso_writer_stamp_returns_raw_weight() {
    let now = 1_000 * DAY_MILLIS;
    // Seconds precision with a trailing `Z`, the shape the MCP clock emits.
    let iso = "2026-02-25T12:34:56Z";
    let w = effective_weight(0.42, iso, now, 30.0);
    assert!((w - 0.42).abs() < 1e-12, "ISO stamp decayed: {w}");
}

#[test]
fn effective_weight_one_half_life_halves() {
    let now = 2_000 * DAY_MILLIS;
    // Δt is exactly one half-life (30 days, half-life 30.0), so the factor is
    // exp(-λ·Δt) = 0.5. The sibling half-life oracle decays weight 1.0, so
    // only a non-unit weight proves the weight is multiplied by the factor.
    let last = now - 30 * DAY_MILLIS;
    let w = effective_weight(0.8, &last.to_string(), now, 30.0);
    assert!((w - 0.8 * 0.5).abs() < 1e-9, "got {w}, want {}", 0.8 * 0.5);
}

#[test]
fn effective_weight_long_age_decays_toward_zero() {
    let now = 3_000 * DAY_MILLIS;
    let half_life_days = 30.0;
    // 10 half-lives (300 days): factor exp(-10·ln2) ≈ 0.001 — small but
    // strictly positive.
    let last = now - 300 * DAY_MILLIS;
    let w = effective_weight(1.0, &last.to_string(), now, half_life_days);
    assert!(
        w > 0.0,
        "ten half-lives should leave a positive weight: {w}"
    );
    assert!(w < 0.01, "ten half-lives should leave almost nothing: {w}");
}

#[test]
fn effective_weight_product_clamps_to_unit_interval() {
    let now = 4_000 * DAY_MILLIS;
    // Zero elapsed leaves the factor at 1.0, so the raw weight is clamped.
    assert!((effective_weight(1.5, &now.to_string(), now, 30.0) - 1.0).abs() < 1e-12);
    assert!((effective_weight(-0.5, &now.to_string(), now, 30.0) - 0.0).abs() < 1e-12);
}

#[test]
fn should_prune_is_strictly_below_w_min() {
    let w_min = 0.05;
    // The 0.049/0.05 and 0.05/0.05 pairs are asserted by the sibling
    // numeric-oracle module; keep the boundaries it does not.
    assert!(!should_prune(0.051, w_min), "above the floor must keep");
    // Boundary holds at 0 too.
    assert!(!should_prune(0.0, 0.0));
    assert!(should_prune(-0.1, 0.0));
}
