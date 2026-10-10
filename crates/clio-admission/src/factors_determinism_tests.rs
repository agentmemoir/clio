// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the admission score is a pure
//! function of its inputs: the same signals must yield the same bits, whatever
//! order the neighbors arrive in and however many times the score is computed.
//!
//! ## Owns
//! - Order-independence of the novelty term over the neighbor set
//! - Repeatability of `weighted_score` for identical factors
//! - Exactness of the f64 round trip through the SQLite `REAL` column
//!
//! ## Does not own
//! - The factor formulas themselves (`factors_tests.rs`)
//! - Gate and threshold policy (`policy_tests.rs`, `decision_tests.rs`)
//! - The signal source that supplies the factors (`clio-mcp`)
//!
//! ## Boundary
//!
//! The arithmetic here is deterministic for fixed inputs, so it could not
//! explain a reported `admission_score` that differed between identical writes.
//! That cause was the signal source, which fed the recency factor an age
//! measured against the write clock; the store-backed source now reports no
//! age. These tests still pin the arithmetic invariant, so a future change that
//! reintroduces order dependence or an f32 step fails here instead of showing
//! up as items swapping ranks in recall.
//!
//! Keep this module focused on score determinism.

// Bit-exact float comparison is the assertion, not an accident: these tests
// exist to catch a last-digit drift, which an epsilon comparison cannot see.
// The f32 cast is deliberate too — it is the narrowing step the tests exist to
// detect, so the lint that would forbid it is the one being tested against.
#![allow(clippy::float_cmp, clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::unreadable_literal)]

use super::{novelty_jaccard, weighted_score};

/// A value that survives the store round trip unchanged.
///
/// The stored column is SQLite `REAL`, which is an f64, so a full-precision
/// f64 must come back bit-identical. Any narrowing to f32 anywhere between the
/// score and the row would show up here as a mismatch.
const FULL_PRECISION_TOLERANCE: f64 = 0.0;

#[test]
fn novelty_ignores_the_order_of_the_neighbor_set() {
    let text = "the staging deploy retries on ECONNRESET with exponential backoff";
    let neighbors = vec![
        "deploy checklist for the staging cluster".to_owned(),
        "the deploy pipeline was reseeded".to_owned(),
        "unrelated note about tomato varieties".to_owned(),
        "backoff settings live in the retry policy".to_owned(),
    ];
    let forward = novelty_jaccard(text, &neighbors);
    let reversed = novelty_jaccard(text, &neighbors.iter().rev().cloned().collect::<Vec<_>>());
    // Rotating changes which neighbor is seen first without changing the set.
    let rotated = novelty_jaccard(
        text,
        &[
            neighbors[2].clone(),
            neighbors[3].clone(),
            neighbors[0].clone(),
            neighbors[1].clone(),
        ],
    );
    assert_eq!(forward, reversed, "novelty depends on neighbor order");
    assert_eq!(forward, rotated, "novelty depends on neighbor order");
}

#[test]
fn novelty_is_stable_across_repeated_calls() {
    let text = "Postgres ts_rank_cd scoring differs from sqlite bm25 ranking";
    let neighbors = vec![
        "bm25 term weighting in the sqlite fts index".to_owned(),
        "ts_rank_cd needs a normalisation value".to_owned(),
    ];
    let first = novelty_jaccard(text, &neighbors);
    for call in 0..64 {
        assert_eq!(
            novelty_jaccard(text, &neighbors),
            first,
            "novelty drifted on call {call}"
        );
    }
}

#[test]
fn weighted_score_is_bit_identical_across_repeats() {
    let weights = [0.25, 0.25, 0.25, 0.25];
    // Values chosen to be unrepresentable in f32, so a narrowing step anywhere
    // in the sum would change the result.
    let factors = [
        0.7582352941176471_f64,
        0.1_f64 / 3.0,
        0.2_f64 / 7.0,
        0.3_f64 / 11.0,
    ];
    let first = weighted_score(weights, factors);
    for call in 0..64 {
        assert_eq!(
            weighted_score(weights, factors),
            first,
            "weighted score drifted on call {call}"
        );
    }
    // And it must not have been rounded to f32 on the way out. The gap below is
    // about 7e-9, comfortably wider than the tolerance, so a narrowing step
    // would be caught rather than absorbed.
    assert_eq!(first.to_string(), "0.21185319582378406");
    assert!(
        (first - (first as f32) as f64).abs() > FULL_PRECISION_TOLERANCE,
        "score is representable as f32, so this fixture would not catch narrowing"
    );
}

#[test]
fn weighted_score_matches_the_same_terms_summed_by_hand() {
    // The score is a plain left-to-right sum of four products. Pinning the
    // arithmetic means a future change to the order, or to a fused multiply-add,
    // shows up as a value change rather than as a silent last-digit drift.
    let weights = [0.25, 0.25, 0.25, 0.25];
    let factors = [0.9, 0.7, 0.55, 0.42];
    let expected = weights[0] * factors[0]
        + weights[1] * factors[1]
        + weights[2] * factors[2]
        + weights[3] * factors[3];
    assert_eq!(weighted_score(weights, factors), expected);
}
