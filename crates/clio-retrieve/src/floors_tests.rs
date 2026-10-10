// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the score-floor unit contract: parsing the
//! optional `min_scores` argument, rejecting values that cannot be honored,
//! and the four floor predicates (arm-local arms, reranker, returned page).
//!
//! ## Owns
//! - `MinScores` validation and `min_scores_from_value` parse assertions,
//!   including the rejection of a mistyped field name
//! - The no-op (absent floor) and inclusive-boundary behavior of every predicate
//! - The `semantic` floor's use of the documented similarity scale
//!
//! ## Does not own
//! - Where a floor runs in the pipeline (`floors_integration_tests`)
//! - The entity-inclusion controls (`entity_controls_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the floor contract itself.

use crate::floors::{
    MAX_FLOOR, MinScores, apply_keyword_floor, apply_reranker_floor, apply_semantic_floor,
    final_floor_keeps, min_scores_from_value,
};
use crate::fusion::StageHit;
use clio_types::ErrorCode;
use serde_json::json;

/// Arm candidates at the distances the dense leg would report: an exact match,
/// a mid-distance pair, and an anti-parallel pair.
fn dense_leg() -> Vec<StageHit> {
    vec![
        StageHit {
            id: "exact".into(),
            score: 0.0,
        },
        StageHit {
            id: "mid".into(),
            score: 0.5,
        },
        StageHit {
            id: "far".into(),
            score: 2.0,
        },
    ]
}

fn ids(hits: &[StageHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.id.as_str()).collect()
}

#[test]
fn the_default_set_is_a_no_op_that_validates() {
    let scores = MinScores::default();
    assert!(scores.is_empty(), "no floor is set by default");
    scores.validate().expect("the empty set is always valid");
    assert_eq!(
        scores,
        min_scores_from_value(None).expect("absent argument")
    );
    assert_eq!(
        scores,
        min_scores_from_value(Some(&json!(null))).expect("null argument"),
        "an explicit null is the same as omitting the argument"
    );
}

#[test]
fn the_documented_object_parses_onto_the_four_named_fields() {
    let value = json!({"semantic": 0.5, "keyword": 1.25, "reranker": 0.2, "final": 0.01});
    let scores = min_scores_from_value(Some(&value)).expect("valid object");
    assert_eq!(scores.semantic, Some(0.5));
    assert_eq!(scores.keyword, Some(1.25));
    assert_eq!(scores.reranker, Some(0.2));
    assert_eq!(scores.final_score, Some(0.01));

    // A partial object is legal: an unset field is simply no floor.
    let partial = min_scores_from_value(Some(&json!({"final": 0.5}))).expect("partial object");
    assert_eq!(partial.final_score, Some(0.5));
    assert!(partial.semantic.is_none());
    assert!(partial.keyword.is_none());
    assert!(partial.reranker.is_none());
}

#[test]
fn a_malformed_object_is_rejected() {
    for value in [
        json!("0.5"),
        json!([0.5]),
        json!({"semantic": "high"}),
        json!({"semantic": null, "keyword": true}),
        json!({"final": {"value": 1}}),
    ] {
        let error = min_scores_from_value(Some(&value))
            .expect_err("a malformed min_scores is never accepted");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{value}");
    }
}

#[test]
fn a_mistyped_field_name_is_rejected_and_never_read_as_no_floor() {
    // The published `retrieve` schema sets `additionalProperties: false` on this
    // object, so a name outside the four must fail closed. Silently dropping it
    // would return an unfiltered page to a caller that believes the floor ran.
    for name in [
        "semantic_typo",
        "finall",
        "final_score",
        "Semantic",
        "min_scores",
    ] {
        let value = json!({name: 0.5});
        let error = min_scores_from_value(Some(&value))
            .expect_err("a mistyped field name is never accepted");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{name}: {error}");
        assert!(
            error.message.contains("unknown field"),
            "{name}: the message must name the problem: {error}"
        );
    }
    // The four documented names, including the renamed `final`, all still parse,
    // so the rejection is about unknown keys and not about the wire spelling.
    min_scores_from_value(Some(&json!({"final": 0.5}))).expect("`final` is the wire name");
    min_scores_from_value(Some(&json!({"semantic": 0.5, "keyword": 0.5})))
        .expect("documented names parse");
}

#[test]
fn a_negative_or_non_finite_floor_is_rejected() {
    for floor in [-0.1, -1.0] {
        let error = MinScores {
            final_score: Some(floor),
            ..MinScores::default()
        }
        .validate()
        .expect_err("a negative floor is never accepted");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{floor}");
    }
    for floor in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = MinScores {
            keyword: Some(floor),
            ..MinScores::default()
        }
        .validate()
        .expect_err("a non-finite floor is never accepted");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{floor}");
    }
}

#[test]
fn a_floor_above_its_documented_range_is_rejected() {
    // `semantic` and `reranker` live in [0, 1], so a larger value could never
    // be met; the unbounded scales are rejected only far above any real score.
    for (name, floor) in [
        (
            "semantic",
            MinScores {
                semantic: Some(1.5),
                ..MinScores::default()
            },
        ),
        (
            "reranker",
            MinScores {
                reranker: Some(2.0),
                ..MinScores::default()
            },
        ),
        (
            "keyword",
            MinScores {
                keyword: Some(MAX_FLOOR + 1.0),
                ..MinScores::default()
            },
        ),
        (
            "final",
            MinScores {
                final_score: Some(MAX_FLOOR * 10.0),
                ..MinScores::default()
            },
        ),
    ] {
        let error = floor.validate().expect_err("out-of-range floor");
        assert_eq!(error.code, ErrorCode::OutOfRange, "{name}");
        assert!(error.message.contains(name), "{name}: {error}");
    }
    // The boundary values themselves are accepted.
    MinScores {
        semantic: Some(1.0),
        reranker: Some(1.0),
        keyword: Some(MAX_FLOOR),
        final_score: Some(0.0),
    }
    .validate()
    .expect("the documented maximums are valid floors");
}

#[test]
fn an_absent_floor_returns_the_list_untouched() {
    assert_eq!(
        ids(&apply_semantic_floor(dense_leg(), None)),
        ["exact", "mid", "far"]
    );
    let lexical = dense_leg();
    assert_eq!(
        ids(&apply_keyword_floor(lexical.clone(), None)),
        ["exact", "mid", "far"]
    );
    let reranked: Vec<(String, Option<f64>)> = vec![("A".into(), Some(0.9))];
    assert_eq!(
        apply_reranker_floor(reranked.clone(), None, |hit| hit.1),
        reranked
    );
    assert!(final_floor_keeps(0.0, None), "no floor keeps every score");
}

#[test]
fn the_semantic_floor_compares_the_documented_similarity_scale() {
    // The leg reports distances 0.0 / 0.5 / 2.0, which the documented transform
    // maps to similarities 1.0 / 0.5 / 0.0 (the last one clamped, never
    // negative). The floor is compared on the similarity, not the distance.
    assert_eq!(
        ids(&apply_semantic_floor(dense_leg(), Some(0.5))),
        ["exact", "mid"],
        "inclusive at the floor; the anti-parallel pair clamps to 0.0 and drops"
    );
    assert_eq!(
        ids(&apply_semantic_floor(dense_leg(), Some(1.0))),
        ["exact"],
        "only an exact match clears a 1.0 floor"
    );
    assert_eq!(
        ids(&apply_semantic_floor(dense_leg(), Some(0.0))),
        ["exact", "mid", "far"],
        "a 0.0 floor keeps the clamped-to-zero candidate, inclusive at the floor"
    );
    assert_eq!(
        ids(&apply_semantic_floor(dense_leg(), Some(0.0001))),
        ["exact", "mid"],
        "a small positive floor still leaves the two real matches"
    );
}

#[test]
fn the_keyword_floor_compares_the_raw_backend_score() {
    // The lexical arm's raw value is higher-is-better on every backend, and the
    // floor is compared against it with no transform.
    assert_eq!(
        ids(&apply_keyword_floor(dense_leg(), Some(0.5))),
        ["mid", "far"],
        "inclusive at the floor, and no transform is applied"
    );
    assert_eq!(ids(&apply_keyword_floor(dense_leg(), Some(1.5))), ["far"]);
    assert!(
        apply_keyword_floor(dense_leg(), Some(2.1)).is_empty(),
        "a floor above the best raw score empties the arm"
    );
}

#[test]
fn the_reranker_floor_keeps_candidates_the_reranker_never_scored() {
    let candidates: Vec<(&str, Option<f64>)> =
        vec![("high", Some(0.9)), ("low", Some(0.2)), ("unscored", None)];
    let kept: Vec<&str> = apply_reranker_floor(candidates, Some(0.5), |hit| hit.1)
        .into_iter()
        .map(|hit| hit.0)
        .collect();
    assert_eq!(
        kept,
        ["high", "unscored"],
        "absence is not a score, so an unscored candidate survives"
    );
    assert_eq!(kept.len(), 2, "the below-floor candidate is removed");
}

#[test]
fn the_final_floor_keeps_scores_at_or_above_it() {
    assert!(
        final_floor_keeps(0.02, Some(0.02)),
        "inclusive at the floor"
    );
    assert!(final_floor_keeps(0.03, Some(0.02)));
    assert!(!final_floor_keeps(0.019, Some(0.02)));
}
