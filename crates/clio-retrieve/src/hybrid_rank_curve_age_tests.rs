// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `age_factor` carried on the fused hits
//! of the rank shaping stage: the carried value equals the factor the curve
//! applied, graph-expanded hits report their own factor, the disabled curve
//! reports the neutral `1.0`, and the value stays inside the floor boundary.
//!
//! ## Owns
//! - Pure `fuse_candidates` assertions over the carried `age_factor`, with
//!   exact bit equality and a fixed clock
//!
//! ## Does not own
//! - The shared shaping fixtures (`hybrid_rank_test_support`)
//! - The curve score arithmetic and its fail-soft boundaries
//!   (`hybrid_rank_curve_tests`)
//! - The reported `scores.age_factor` contract end to end
//!   (`stage_score_age_tests`, `finalize_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the carried age factor and its boundaries.

use super::hybrid_rank_test_support::{
    FIXED_NOW, FLOOR, HUGE_OLD, active_curve, disabled_curve, fixture_item, fixture_stamps,
    fixture_weights, one_year_factor, shape_fixture,
};
use super::*;
use crate::fusion::DEFAULT_RRF_K;
use clio_config::ranking::{DEFAULT_RETENTION_HALF_LIFE_DAYS, TrustRatios};
use clio_types::decay::floored_half_life_factor;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn carried_age_factor_equals_the_applied_factor() {
    let disabled = shape_fixture(disabled_curve());
    let active = shape_fixture(active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS));
    for hit in &active {
        let base = disabled
            .iter()
            .find(|b| b.id == hit.id)
            .expect("paired hit");
        // The carried value is the exact multiplier the curve applied.
        assert_eq!(
            hit.score.to_bits(),
            (base.score * hit.age_factor).to_bits(),
            "{}",
            hit.id
        );
    }
    for (id, stamp) in fixture_stamps() {
        let hit = active.iter().find(|h| h.id == id).expect("fixture hit");
        let age = crate::filter::age_days(stamp, FIXED_NOW).expect("parseable stamp");
        assert_eq!(
            hit.age_factor.to_bits(),
            floored_half_life_factor(age, DEFAULT_RETENTION_HALF_LIFE_DAYS, FLOOR).to_bits(),
            "{id}"
        );
    }
}

#[test]
fn graph_expanded_aged_hit_reports_its_own_factor() {
    let active = shape_fixture(active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS));
    // D joins only through graph expansion and is then factored exactly once,
    // so the factor it reports is the one that was applied to it.
    let expanded = active.iter().find(|h| h.id == "D").expect("D hit");
    assert_eq!(
        expanded.age_factor.to_bits(),
        one_year_factor().to_bits(),
        "D factor {}",
        expanded.age_factor
    );
}

#[test]
fn curve_disabled_reports_the_neutral_age_factor() {
    let fused = shape_fixture(disabled_curve());
    assert_eq!(fused.len(), 4, "the loop must cover every fused hit");
    for hit in &fused {
        assert_eq!(hit.age_factor.to_bits(), 1.0f64.to_bits(), "{}", hit.id);
    }
}

#[test]
fn age_factor_stays_within_the_floor_boundary() {
    // A hit old enough to underflow lands exactly on the floor.
    let items = BTreeMap::from([("H".to_owned(), fixture_item("H", 0.5, HUGE_OLD))]);
    let dense = [StageHit {
        id: "H".into(),
        score: 0.9,
    }];
    let fused = fuse_candidates(
        &fixture_weights(),
        DEFAULT_RRF_K,
        &dense,
        &[],
        &[],
        &items,
        &BTreeSet::new(),
        active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS),
        TrustRatios::default(),
    );
    assert_eq!(fused[0].age_factor.to_bits(), FLOOR.to_bits());
    // Across the fixture every reported factor stays in `[floor, 1]`, and a
    // floor of exactly 1.0 clamps every factor to exactly 1.0.
    for (floor, clamped) in [(FLOOR, None), (1.0, Some(1.0f64.to_bits()))] {
        for hit in shape_fixture(active_curve(floor, DEFAULT_RETENTION_HALF_LIFE_DAYS)) {
            if let Some(bits) = clamped {
                assert_eq!(
                    hit.age_factor.to_bits(),
                    bits,
                    "{} at floor {floor}",
                    hit.id
                );
            }
            assert!(
                (floor..=1.0).contains(&hit.age_factor),
                "{}: {} outside [{floor}, 1]",
                hit.id,
                hit.age_factor
            );
        }
    }
}
