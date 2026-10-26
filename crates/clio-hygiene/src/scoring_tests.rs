// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests deterministic noise-score composition (`scoring.rs`).
//!
//! ## Owns
//! - Test coverage for appendix weight composition, graded credential scoring,
//!   the token-run proxy band, and suggested-action bands
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
        secret_kinds: vec![],
        secret_run_len: 0,
        token_run_len: None,
        pattern_kinds: vec![],
        near_duplicate: false,
        utility_proxy: 1.0,
        staleness: 0.0,
    }
}

/// Score of one credential kind at the kind base alone (no measured run).
fn secret_score(kind: &'static str) -> f64 {
    let mut i = input();
    i.secret_kinds = vec![kind];
    i.secret_run_len = 0;
    score_noise(&i, &HygieneWeights::default()).score
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
fn named_credential_key_is_a_discard_credential() {
    let mut i = input();
    i.secret_kinds = vec!["named_credential_key"];
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!(
        scored.score >= 0.90,
        "a named credential key reaches the credential floor"
    );
    assert_eq!(scored.reasons[0], "secret_credential");
    assert!(scored.reasons.contains(&"named_credential_key"));
    assert_eq!(scored.suggested_action, Some("discard"));
}

#[test]
fn secret_scores_vary_by_kind() {
    let pem = secret_score("pem_private_key");
    let known = secret_score("known_secret_format");
    let named = secret_score("named_credential_key");
    let bearer = secret_score("bearer_token");
    assert!(pem > known, "pem outranks known secret format");
    assert!(
        known > named,
        "known secret format outranks named credential key"
    );
    assert!(named > bearer, "named credential key outranks bearer token");
}

#[test]
fn secret_score_grows_with_run_length() {
    let mut short = input();
    short.secret_kinds = vec!["known_secret_format"];
    short.secret_run_len = 16;
    let mut long = short.clone();
    long.secret_run_len = 64;
    let short_score = score_noise(&short, &HygieneWeights::default()).score;
    let long_score = score_noise(&long, &HygieneWeights::default()).score;
    assert!(
        long_score > short_score,
        "a 64-byte credential run outranks a 16-byte run"
    );
}

#[test]
fn token_shaped_run_is_not_a_credential_band() {
    let mut short = input();
    short.token_run_len = Some(24);
    let mut long = input();
    long.token_run_len = Some(120);
    let short_hit = score_noise(&short, &HygieneWeights::default());
    let long_hit = score_noise(&long, &HygieneWeights::default());
    for scored in [&short_hit, &long_hit] {
        assert!(
            (0.40..0.70).contains(&scored.score),
            "proxy score sits in the flag band"
        );
        assert!(!scored.reasons.contains(&"secret_credential"));
        assert!(scored.reasons.contains(&"token_shaped_run"));
        assert_eq!(scored.suggested_action, Some("flag"));
    }
    assert!(
        long_hit.score > short_hit.score,
        "a 120-char token run outranks a 24-char run"
    );
}

#[test]
fn token_shaped_run_with_other_signals_stays_flag() {
    let mut i = input();
    i.token_run_len = Some(120);
    i.pattern_kinds = vec!["terminal_spam", "stack_trace"];
    i.near_duplicate = true;
    i.utility_proxy = 0.0;
    i.staleness = 1.0;
    let scored = score_noise(&i, &HygieneWeights::default());
    assert!(
        scored.score < 0.70,
        "a proxy-only score stays under the discard band"
    );
    assert_eq!(scored.suggested_action, Some("flag"));
    assert!(!scored.reasons.contains(&"secret_credential"));
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
    // renormalized to .5/.5: the weight-scaled strength lands at .465, so the
    // credential floor (named key base .93, no run length) still governs
    let mut i = input();
    i.secret_kinds = vec!["named_credential_key"];
    let scored = score_noise(&i, &weights);
    assert!(
        (scored.score - 0.93).abs() < 1e-9,
        "credential floor still applies"
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
