// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the payload-level `age_factor` contract
//! tests: the numbers-or-null security property over the whole documented
//! `scores` key set, and the finiteness and range of `age_factor` against the
//! request's retention floor.
//!
//! ## Owns
//! - `scores_object_carries_only_numbers_and_nulls`, the security property
//!   over every documented `scores` key
//! - `age_factor_is_a_finite_number_within_floor_and_one`, the payload-level
//!   range and finiteness assertion
//!
//! ## Does not own
//! - Score computation or the dense transform (`clio-retrieve` stage scoring)
//! - The binding parity and populated-`semantic` tests (`read_scores_tests`)
//!
//! ## Boundary
//!
//! Changes to what the fixtures contain should normally require changes in
//! the shared support module (`read_scores_test_support.rs`).
//! Changes to the `age_factor` range should normally require changes here.
//!
//! Keep this module focused on the payload-level `age_factor` contract.

use super::read_scores_test_support::{base_item, call, hits, put, scores_of, state};
use clio_config::retention::DEFAULT_RETENTION_FLOOR;
use serde_json::json;

/// Security: every value in the documented `scores` key set — including the
/// newest key, `age_factor` — is a number or null, so no content or query
/// text can ride along in the object.
#[test]
fn scores_object_carries_only_numbers_and_nulls() {
    let state = state();
    put(
        &state,
        &base_item("S", "bank-a", "secret gist content"),
        "secret gist content",
    );

    let query = "secret";
    let out = call(
        &state,
        "retrieve",
        json!({"bank": "bank-a", "query": query}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    for hit in hits(&out) {
        let scores = scores_of(hit);
        for (key, value) in scores {
            assert!(
                value.is_number() || value.is_null(),
                "`{key}` must be number or null: {hit}"
            );
        }
        let rendered = serde_json::to_string(scores).expect("serialize scores");
        assert!(
            !rendered.contains(query),
            "no query text in scores: {rendered}"
        );
        assert!(
            !rendered.contains("secret gist content"),
            "no content text in scores: {rendered}"
        );
    }
}

/// Payload contract: every hit's `age_factor` is a finite number in
/// `[retention_floor, 1]` for the request's floor. The state fixture carries
/// the builtin default profile, whose floor is `DEFAULT_RETENTION_FLOOR`, and
/// the floor clamps any non-negative age, so the assertion holds whatever the
/// engine's clock reads.
#[test]
fn age_factor_is_a_finite_number_within_floor_and_one() {
    let state = state();
    put(
        &state,
        &base_item("F", "bank-a", "age factor range check"),
        "age factor range check",
    );

    let out = call(
        &state,
        "retrieve",
        json!({"bank": "bank-a", "query": "age factor"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    for hit in hits(&out) {
        let factor = scores_of(hit)["age_factor"]
            .as_f64()
            .unwrap_or_else(|| panic!("numeric age_factor: {hit}"));
        assert!(factor.is_finite(), "finite age_factor: {hit}");
        assert!(
            (DEFAULT_RETENTION_FLOOR..=1.0).contains(&factor),
            "age factor within [floor, 1]: {hit}"
        );
    }
}
