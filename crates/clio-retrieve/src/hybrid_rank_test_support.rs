// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the shaping fixtures shared by the two `hybrid_rank`
//! test modules: the fixture legs, the fixture items, the shared stamps, and
//! the fused-hit accessors.
//!
//! ## Owns
//! - `fixture_legs`, the dense and lexical legs whose fusion order is B, A, C
//! - `fixture_item` and `fixture_items`, the fetched items with their
//!   admission scores and stamps
//! - `ids` and `score_of`, the fused-hit accessors
//! - `FIXED_NOW`, `ONE_YEAR_OLD`, and `TEN_YEARS_OLD`, the shared stamps
//! - The retention-curve arguments and the shaped four-item fixture
//!   (`fixture_weights`, `active_curve`, `disabled_curve`, `shape_fixture`,
//!   `shape_fixture_trust`, `one_year_factor`, `fixture_stamps`, `FLOOR`,
//!   `HUGE_OLD`) shared by the curve test modules
//!
//! ## Does not own
//! - Any test
//! - The fusion and shaping code under test (`hybrid_rank.rs`)
//! - Module-only setup: the captured byte baseline and the single-hit helper
//!   stay in the curve module that owns them
//!
//! ## Boundary
//!
//! Changes to what the fixtures contain should normally require changes here.
//! Changes to how a single test asserts should not require changes here.
//!
//! Keep this module focused on the shared shaping fixtures.

use super::{RetentionArgs, fuse_candidates};
use crate::fixtures_tests as fx;
use crate::fusion::{DEFAULT_RRF_K, FusedHit, StageHit};
use clio_config::ranking::{DEFAULT_RETENTION_HALF_LIFE_DAYS, RetrievalWeights, TrustRatios};
use clio_types::MemoryItem;
use clio_types::decay::floored_half_life_factor;
use std::collections::{BTreeMap, BTreeSet};

/// The fixed clock every age and fixture stamp is measured against; a stamp
/// at it has age exactly zero.
pub(super) const FIXED_NOW: &str = "2026-01-01T00:00:00Z";
/// Exactly one year before the fixed clock: 365 days.
pub(super) const ONE_YEAR_OLD: &str = "2025-01-01T00:00:00Z";
/// Ten years before the fixed clock, so the decaying share of the age factor
/// is small.
pub(super) const TEN_YEARS_OLD: &str = "2016-01-01T00:00:00Z";

/// Two legs whose fusion order is B, A, C before any shaping.
///
/// With both leg weights equal the list weights are 0.5 each, so at
/// `k = 60`: A is dense rank 1 only, B is dense rank 2 and lexical rank 1, and C
/// is lexical rank 2 only.
pub(super) fn fixture_legs() -> (Vec<StageHit>, Vec<StageHit>) {
    let hit = |id: &str, score: f64| StageHit {
        id: id.into(),
        score,
    };
    (
        vec![hit("A", 0.9), hit("B", 0.8)],
        vec![hit("B", 12.0), hit("C", 11.0)],
    )
}

/// One fetched item with an explicit admission score; the stamp goes to
/// `created_at`, `updated_at`, and `valid_from`.
pub(super) fn fixture_item(id: &str, admission: f64, stamp: &str) -> MemoryItem {
    let mut item = fx::item(id, "bank", "gist");
    item.admission_score = Some(admission);
    item.created_at = stamp.to_owned();
    item.updated_at = stamp.to_owned();
    item.valid_from = stamp.to_owned();
    item
}

/// The four items the fixture legs fuse, plus the graph-expanded id: A carries
/// admission 0.8, B 0.2, C 0.5, and D 0.6, each stamped per the call.
pub(super) fn fixture_items(stamps: [(&str, &str); 4]) -> BTreeMap<String, MemoryItem> {
    let admission = |id: &str| match id {
        "A" => 0.8,
        "B" => 0.2,
        "C" => 0.5,
        "D" => 0.6,
        other => panic!("no fixture admission score for {other}"),
    };
    stamps
        .into_iter()
        .map(|(id, stamp)| (id.to_owned(), fixture_item(id, admission(id), stamp)))
        .collect()
}

/// The ids of the fused hits, in the returned order.
pub(super) fn ids(hits: &[FusedHit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

/// The fused score of one fixture id.
pub(super) fn score_of(hits: &[FusedHit], id: &str) -> f64 {
    hits.iter()
        .find(|h| h.id == id)
        .unwrap_or_else(|| panic!("no fused hit for {id}"))
        .score
}

/// Far enough back that the exponential underflows to exactly zero.
pub(super) const HUGE_OLD: &str = "0001-01-01T00:00:00Z";
/// The floor the curve tests exercise, matching the request default.
pub(super) const FLOOR: f64 = 0.25;

/// The one graph-expanded-only fixture id.
pub(super) fn fixture_expanded() -> BTreeSet<String> {
    BTreeSet::from(["D".to_owned()])
}

/// Fixed, equal leg weights with a nonzero importance weight.
pub(super) fn fixture_weights() -> RetrievalWeights {
    RetrievalWeights {
        dense: 1.0,
        lexical: 1.0,
        importance: 0.25,
    }
}

/// The curve active over the fixed clock.
pub(super) fn active_curve(floor: f64, half_life_days: f64) -> RetentionArgs<'static> {
    RetentionArgs {
        active: true,
        floor,
        half_life_days,
        now: FIXED_NOW,
    }
}

/// The curve disabled at the fixed clock: the pre-curve call shape.
///
/// The floor and the half-life are valid on purpose. `active = false` makes the
/// curve ignore them, so dropping the active guard would factor the aged
/// fixture and change the pre-curve bytes. Invalid values here would hide that
/// behind the knob check instead.
pub(super) fn disabled_curve() -> RetentionArgs<'static> {
    RetentionArgs {
        active: false,
        floor: FLOOR,
        half_life_days: DEFAULT_RETENTION_HALF_LIFE_DAYS,
        now: FIXED_NOW,
    }
}

/// The fused order and scores of the pre-curve fixture under `retention`.
///
/// The fixture stamps carry the curve's spread: A is ten years old, B is
/// fresh, and C and D are one year old, so the curve moves different hits by
/// different amounts. The fixture items are facts, so the default trust
/// environment leaves every multiplier neutral.
pub(super) fn shape_fixture(retention: RetentionArgs<'_>) -> Vec<FusedHit> {
    shape_fixture_trust(retention, TrustRatios::default())
}

/// The same fused fixture under an explicit trust environment.
pub(super) fn shape_fixture_trust(
    retention: RetentionArgs<'_>,
    trust: TrustRatios,
) -> Vec<FusedHit> {
    let (dense, lexical) = fixture_legs();
    fuse_candidates(
        &fixture_weights(),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &fixture_items([
            ("A", TEN_YEARS_OLD),
            ("B", FIXED_NOW),
            ("C", ONE_YEAR_OLD),
            ("D", ONE_YEAR_OLD),
        ]),
        &fixture_expanded(),
        retention,
        trust,
    )
}

/// The factor both one-year-old fixture items must receive.
pub(super) fn one_year_factor() -> f64 {
    floored_half_life_factor(365.0, DEFAULT_RETENTION_HALF_LIFE_DAYS, FLOOR)
}

/// The stamps of the four fixture items, with their ids.
pub(super) fn fixture_stamps() -> [(&'static str, &'static str); 4] {
    [
        ("A", TEN_YEARS_OLD),
        ("B", FIXED_NOW),
        ("C", ONE_YEAR_OLD),
        ("D", ONE_YEAR_OLD),
    ]
}
