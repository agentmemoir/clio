// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `age_factor` reporting-contract tests:
//! the reported factor is exactly the factor the retention curve applied
//! fuse → shape → finalize, the disabled curve reports the neutral `1.0`, and
//! the serialized value is a finite number inside `[floor, 1.0]`.
//!
//! ## Owns
//! - End-to-end assertions over `scores.age_factor` with exact bit equality
//!   and the request's fixed clock
//!
//! ## Does not own
//! - The curve arithmetic, its boundaries, and the carried value on the fused
//!   hit (`hybrid_rank_curve_tests`)
//! - The neutral initialization at fusion (`fusion_tests`) and the carried
//!   read at finalize (`finalize_tests`)
//! - The serialized key-set and null-when-absent rules (`stage_score_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::types::{RetrieveOutcome, RetrieveRequest};
use clio_config::ranking::DEFAULT_RETENTION_HALF_LIFE_DAYS;
use clio_config::retention::DEFAULT_RETENTION_FLOOR;
use clio_types::decay::floored_half_life_factor;

/// Request with a frozen clock one day after the fixture stamp, so the curve's
/// age is exactly one day.
fn request(curve: bool) -> RetrieveRequest {
    let mut req = RetrieveRequest::new("bank", "alpha");
    req.now = Some("2026-01-02T00:00:00Z".into());
    req.retention_curve = curve;
    req
}

/// Retrieve one lexical-only fixture item stamped 2026-01-01 with the clock
/// frozen one day later, returning its outcome.
fn retrieve(curve: bool) -> RetrieveOutcome {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha memory A");
    fx::put_lexical(&store, &entry, "alpha memory token A");
    let retriever = fx::retriever(&store);
    retriever.retrieve(&request(curve)).expect("retrieve")
}

/// The factor the curve applies to a one-day-old hit.
fn one_day_factor() -> f64 {
    floored_half_life_factor(
        1.0,
        DEFAULT_RETENTION_HALF_LIFE_DAYS,
        DEFAULT_RETENTION_FLOOR,
    )
}

#[test]
fn reported_age_factor_equals_the_applied_curve_factor() {
    let off = retrieve(false);
    let on = retrieve(true);
    let base = off
        .hits
        .iter()
        .find(|h| h.hit.item_id == "A")
        .expect("A hit");
    let hit = on
        .hits
        .iter()
        .find(|h| h.hit.item_id == "A")
        .expect("A hit");
    let applied = one_day_factor();
    assert!(applied < 1.0, "fixture must be curve-demoted: {applied}");
    assert_eq!(
        hit.scores.age_factor.to_bits(),
        applied.to_bits(),
        "reported {}",
        hit.scores.age_factor
    );
    // The reported value is the exact multiplier that reached the score.
    assert_eq!(
        hit.score.to_bits(),
        (base.score * hit.scores.age_factor).to_bits()
    );
}

#[test]
fn curve_disabled_reports_the_neutral_age_factor() {
    let out = retrieve(false);
    for hit in &out.hits {
        assert_eq!(
            hit.scores.age_factor.to_bits(),
            1.0f64.to_bits(),
            "{}",
            hit.hit.item_id
        );
    }
}

#[test]
fn serialized_age_factor_is_a_finite_number_in_range() {
    let out = retrieve(true);
    let hit = out.hits.first().expect("one hit");
    let value = serde_json::to_value(hit).expect("serialize hit");
    let reported = &value["scores"]["age_factor"];
    // serde_json writes NaN and infinity as null, so a non-null number here is
    // the proof that the carried factor is finite.
    assert!(
        reported.is_number(),
        "age_factor must serialize as a number, got {reported}"
    );
    let factor = reported.as_f64().expect("finite f64");
    assert!(factor.is_finite(), "age_factor must be finite: {factor}");
    assert!(
        (DEFAULT_RETENTION_FLOOR..=1.0).contains(&factor),
        "factor {factor} outside [floor, 1]"
    );
}
