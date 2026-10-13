// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests deterministic noise-score composition (`scoring.rs`).
//!
//! ## Owns
//! - Test coverage for appendix weight composition, the secret score floor,
//!   and suggested-action bands
//!
//! ## Does not own
//! - Signal detection (`detect_tests.rs`)
//! - The scoring behavior itself (`scoring.rs`)
//!
//! ## Boundary
//!
//! Tests here break when scoring changes; they never change it.
//!
//! Keep this module focused on `scoring.rs` tests.

use crate::scoring::{ScoreInput, score_noise, suggested_action};
use clio_config::ranking::HygieneWeights;

fn input() -> ScoreInput {
    ScoreInput {
        secret_hit: false,
        pattern_kinds: vec![],
        near_duplicate: false,
        utility_proxy: 1.0,
        staleness: 0.0,
    }
}

#[test]
fn defaults_compose_appendix_weights() {
    // defaults: secret .35 pattern .25 dup .15 utility .15 staleness .10
    let mut i = input();
    i.near_duplicate = true;
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!((scored.score - 0.15).abs() < 1e-9);
    assert_eq!(scored.reasons, vec!["near_duplicate"]);
    assert_eq!(scored.suggested_action, None);
}

#[test]
fn identical_inputs_yield_identical_scores() {
    let mut i = input();
    i.pattern_kinds = vec!["terminal_spam"];
    i.utility_proxy = 0.2;
    i.staleness = 0.7;
    let a = score_noise(&i, &HygieneWeights::default());
    let b = score_noise(&i, &HygieneWeights::default());
    assert_eq!(a, b);
}

#[test]
fn secret_hit_floors_score_at_ninety() {
    let mut i = input();
    i.secret_hit = true;
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!(scored.score >= 0.90);
    assert_eq!(scored.reasons[0], "secret_credential");
    assert_eq!(scored.suggested_action, Some("discard"));
}

#[test]
fn action_bands_follow_appendix() {
    assert_eq!(suggested_action(0.95, false), Some("discard"));
    assert_eq!(suggested_action(0.75, false), Some("archive"));
    assert_eq!(suggested_action(0.45, false), Some("flag"));
    assert_eq!(suggested_action(0.10, false), None);
    assert_eq!(
        suggested_action(0.10, true),
        Some("discard"),
        "any secret-class hit forces discard"
    );
}

#[test]
fn weights_are_renormalized() {
    let weights = HygieneWeights {
        secret: 1.0,
        pattern: 0.0,
        dup: 0.0,
        utility: 1.0,
        staleness: 0.0,
    };
    // renormalized to .5/.5: secret hit alone = .5, below floor → keep bands
    let mut i = input();
    i.secret_hit = true;
    let scored = score_noise(&i, &weights);
    assert!(
        (scored.score - 0.90).abs() < 1e-9,
        "secret floor still applies"
    );
}

#[test]
fn utility_gap_and_staleness_raise_score() {
    let mut i = input();
    i.utility_proxy = 0.0;
    i.staleness = 1.0;
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!((scored.score - 0.25).abs() < 1e-9);
    assert!(scored.reasons.contains(&"low_utility"));
    assert!(scored.reasons.contains(&"stale"));
}

#[test]
fn score_is_clamped_to_unit_interval() {
    let mut i = input();
    i.utility_proxy = 0.0;
    i.staleness = 1.0;
    i.near_duplicate = true;
    i.pattern_kinds = vec!["terminal_spam", "command_dump"];
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!(scored.score <= 1.0 && scored.score >= 0.0);
}
