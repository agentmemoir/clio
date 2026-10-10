// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retention-curve tests of the rank
//! shaping stage: the disabled curve reproducing the pre-curve bytes, the
//! floored age factor on fused and graph-expanded hits, the `created_at` age
//! source, and the fail-soft input boundaries.
//!
//! ## Owns
//! - The fixed pre-curve fixture and the captured `id:bits` byte baseline
//! - Pure `fuse_candidates` score assertions over hand-built legs, item maps,
//!   and a fixed clock
//!
//! ## Does not own
//! - The shared shaping fixtures (`hybrid_rank_test_support`)
//! - The `age_factor` carried on the fused hits (`hybrid_rank_curve_age_tests`)
//! - The importance-only shaping and the graph-expansion join with the curve
//!   off (`hybrid_rank_tests`), the fusion formula (`fusion_tests`), the
//!   end-to-end retrieval path (`hybrid_tests`), or the maintenance write that
//!   re-stamps `created_at` on a consolidated rebuild (`clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the age-factor curve and its boundaries.

use super::hybrid_rank_test_support::{
    FIXED_NOW, FLOOR, HUGE_OLD, ONE_YEAR_OLD, TEN_YEARS_OLD, active_curve, disabled_curve,
    fixture_item, fixture_legs, fixture_weights, ids, one_year_factor, score_of, shape_fixture,
};
use super::*;
use crate::fusion::DEFAULT_RRF_K;
use clio_config::ranking::{DEFAULT_RETENTION_HALF_LIFE_DAYS, TrustRatios};
use clio_types::decay::floored_half_life_factor;
use std::collections::{BTreeMap, BTreeSet};

/// The captured pre-curve bytes, in returned order.
const PRE_CURVE_BASELINE: [&str; 4] = [
    "B:4580578553121802251",
    "A:4576823727541414983",
    "C:4576383608445096596",
    "D:4576074953494431755",
];

/// The score one single dense-only hit gets under `retention`; the id only
/// names the hit.
fn single_score(id: &str, created_at: &str, updated_at: &str, retention: RetentionArgs<'_>) -> f64 {
    let mut item = fixture_item(id, 0.8, created_at);
    item.updated_at = updated_at.to_owned();
    let items = BTreeMap::from([(id.to_owned(), item)]);
    let dense = [StageHit {
        id: id.into(),
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
        retention,
        TrustRatios::default(),
    );
    fused[0].score
}

/// Print the shaping output as `id:score.to_bits()` per returned hit, in the
/// returned order. Captured before the retention curve landed; the disabled
/// path must keep printing those exact bytes.
#[test]
fn capture_pre_curve_shaping_baseline() {
    let fused = shape_fixture(disabled_curve());
    assert_eq!(fused.len(), 4, "capture must cover every fixture hit");
    for hit in &fused {
        println!("{}:{}", hit.id, hit.score.to_bits());
    }
}

#[test]
fn retention_disabled_reproduces_the_pre_curve_shaping_bytes() {
    let fused = shape_fixture(disabled_curve());
    let got: Vec<String> = fused
        .iter()
        .map(|h| format!("{}:{}", h.id, h.score.to_bits()))
        .collect();
    assert_eq!(got, PRE_CURVE_BASELINE);
}

#[test]
fn aged_item_ranks_below_identical_fresh_item() {
    let items = BTreeMap::from([
        ("fresh".to_owned(), fixture_item("fresh", 0.5, FIXED_NOW)),
        ("old".to_owned(), fixture_item("old", 0.5, ONE_YEAR_OLD)),
    ]);
    // One rank-one slot per leg: both hits fuse to the same pre-factor score.
    let dense = [StageHit {
        id: "fresh".into(),
        score: 0.9,
    }];
    let lexical = [StageHit {
        id: "old".into(),
        score: 12.0,
    }];
    let fused = fuse_candidates(
        &fixture_weights(),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS),
        TrustRatios::default(),
    );
    assert_eq!(ids(&fused), vec!["fresh", "old"]);
    let age = crate::filter::age_days(ONE_YEAR_OLD, FIXED_NOW).expect("parseable stamp");
    assert_eq!(age.to_bits(), 365.0f64.to_bits());
    assert_eq!(
        score_of(&fused, "old").to_bits(),
        (score_of(&fused, "fresh") * one_year_factor()).to_bits()
    );
    assert!(score_of(&fused, "fresh") > score_of(&fused, "old"));
}

#[test]
fn graph_expanded_aged_neighbour_receives_the_same_factor() {
    let disabled = shape_fixture(disabled_curve());
    let active = shape_fixture(active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS));
    let factor = one_year_factor();
    // C is fused and D is expanded-only; both are one year old, so both carry
    // the same factor, and D is factored exactly once after it joins.
    assert_eq!(
        score_of(&active, "C").to_bits(),
        (score_of(&disabled, "C") * factor).to_bits()
    );
    assert_eq!(
        score_of(&active, "D").to_bits(),
        (score_of(&disabled, "D") * factor).to_bits()
    );
    assert_eq!(
        score_of(&active, "D").to_bits(),
        (score_of(&active, "B") * DEFAULT_GRAPH_DECAY * factor).to_bits()
    );
    // The fresh best keeps its bytes, so the expanded factor is the same one
    // the fused aged hit received.
    assert_eq!(
        score_of(&active, "B").to_bits(),
        score_of(&disabled, "B").to_bits()
    );
}

#[test]
fn age_uses_created_at_not_updated_at() {
    // A consolidated unit's revision write re-stamps `created_at` at every
    // rebuild, so `created_at` is its last-rebuild time; `updated_at` is not
    // read.
    let curve = active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS);
    let base = single_score("base", FIXED_NOW, FIXED_NOW, curve);
    let old_created = single_score("old-created", ONE_YEAR_OLD, FIXED_NOW, curve);
    let old_updated = single_score("old-updated", FIXED_NOW, ONE_YEAR_OLD, curve);
    assert_eq!(old_created.to_bits(), (base * one_year_factor()).to_bits());
    assert_eq!(old_updated.to_bits(), base.to_bits());
}

#[test]
fn unparseable_created_at_leaves_the_score_unchanged() {
    let curve = active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS);
    let base = single_score("base", FIXED_NOW, FIXED_NOW, curve);
    for stamp in ["", "not-a-timestamp"] {
        let got = single_score("bad", stamp, FIXED_NOW, curve);
        assert_eq!(got.to_bits(), base.to_bits(), "stamp {stamp:?}");
    }
}

#[test]
fn invalid_floor_is_ignored_not_zeroing_scores() {
    let base = single_score("base", ONE_YEAR_OLD, ONE_YEAR_OLD, disabled_curve());
    // 1.0 is a valid floor and the factor is exactly one; the others are
    // invalid and must leave scores unchanged rather than zeroing them.
    for floor in [1.0, 0.0, -0.25, f64::NAN, f64::INFINITY, 2.0] {
        let got = single_score(
            "aged",
            ONE_YEAR_OLD,
            ONE_YEAR_OLD,
            active_curve(floor, DEFAULT_RETENTION_HALF_LIFE_DAYS),
        );
        assert_eq!(got.to_bits(), base.to_bits(), "floor {floor}");
    }
}

#[test]
fn invalid_half_life_is_ignored_not_demoted_to_the_floor() {
    let base = single_score("base", ONE_YEAR_OLD, ONE_YEAR_OLD, disabled_curve());
    // A bad half-life leaves the score unchanged; it must not become the floor,
    // which would punish the data for a configuration error.
    for half_life in [0.0, -30.0, f64::NAN, f64::INFINITY] {
        let got = single_score(
            "aged",
            ONE_YEAR_OLD,
            ONE_YEAR_OLD,
            active_curve(FLOOR, half_life),
        );
        assert_eq!(got.to_bits(), base.to_bits(), "half-life {half_life}");
    }
}

#[test]
fn very_large_age_lands_exactly_on_the_floor() {
    let base = single_score("base", FIXED_NOW, FIXED_NOW, disabled_curve());
    let age = crate::filter::age_days(HUGE_OLD, FIXED_NOW).expect("parseable stamp");
    assert!(age > 1.0e5, "age {age} must be past the underflow point");
    assert_eq!(
        floored_half_life_factor(age, DEFAULT_RETENTION_HALF_LIFE_DAYS, FLOOR).to_bits(),
        FLOOR.to_bits(),
        "the exponential underflows to exactly zero"
    );
    let got = single_score(
        "huge",
        HUGE_OLD,
        HUGE_OLD,
        active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS),
    );
    assert_eq!(got.to_bits(), (base * FLOOR).to_bits());
}

#[test]
fn active_curve_leaves_a_hit_with_no_item_unchanged() {
    let (dense, lexical) = fixture_legs();
    // "C" fuses from the lexical leg but was not fetched, so the age loop has
    // no `created_at` to read and leaves its fused score alone. "B" is fetched
    // and one year old, so it is factored and proves the loop ran.
    let items = BTreeMap::from([
        ("A".to_owned(), fixture_item("A", 0.8, FIXED_NOW)),
        ("B".to_owned(), fixture_item("B", 0.2, ONE_YEAR_OLD)),
    ]);
    let run = |retention: RetentionArgs<'static>| {
        fuse_candidates(
            &fixture_weights(),
            DEFAULT_RRF_K,
            &dense,
            &lexical,
            &[],
            &items,
            &BTreeSet::new(),
            retention,
            TrustRatios::default(),
        )
    };
    let disabled = run(disabled_curve());
    let active = run(active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS));
    assert_eq!(
        score_of(&active, "C").to_bits(),
        score_of(&disabled, "C").to_bits()
    );
    assert_eq!(
        score_of(&active, "A").to_bits(),
        score_of(&disabled, "A").to_bits()
    );
    assert_eq!(
        score_of(&active, "B").to_bits(),
        (score_of(&disabled, "B") * one_year_factor()).to_bits()
    );
}

/// Moved from the pre-curve suite: with the curve disabled, exchanging two
/// items' ages changes neither the order nor any score, so no age term reaches
/// the shaping output unless the curve is active.
#[test]
fn the_shaped_score_with_the_curve_off_does_not_depend_on_item_timestamps() {
    let (dense, lexical) = fixture_legs();
    let run = |a_stamp: &'static str, b_stamp: &'static str| {
        let items = BTreeMap::from([
            ("A".to_owned(), fixture_item("A", 0.8, a_stamp)),
            ("B".to_owned(), fixture_item("B", 0.2, b_stamp)),
            ("C".to_owned(), fixture_item("C", 0.5, ONE_YEAR_OLD)),
        ]);
        fuse_candidates(
            &fixture_weights(),
            DEFAULT_RRF_K,
            &dense,
            &lexical,
            &[],
            &items,
            &BTreeSet::new(),
            disabled_curve(),
            TrustRatios::default(),
        )
    };
    let first = run(TEN_YEARS_OLD, FIXED_NOW);
    let second = run(FIXED_NOW, TEN_YEARS_OLD);
    assert_eq!(ids(&first), ids(&second));
    for id in ["A", "B", "C"] {
        assert_eq!(
            score_of(&first, id).to_bits(),
            score_of(&second, id).to_bits(),
            "{id}"
        );
    }
}
