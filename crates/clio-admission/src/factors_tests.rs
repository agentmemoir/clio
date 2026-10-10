// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deterministic factor-signal assertions:
//! utility, confidence, source weighting, and the recency decay curve.
//!
//! ## Owns
//! - Every assertion about the factor functions declared in `factors`
//!
//! ## Does not own
//! - The factor arithmetic itself (see `factors`)
//! - Admission decision and threshold tests (`decision_*_tests`)
//!
//! ## Boundary
//!
//! Changes to a factor's pinned expectations should normally require changes
//! here. Changes to the factor arithmetic should not require changes here
//! beyond recomputing expectations from the formula.
//!
//! Keep this module focused on factor-signal assertions.

use super::*;
use clio_types::ErrorCode;

#[test]
fn utility_markers_and_chatter() {
    assert!((utility_score("I prefer concise output") - 0.9).abs() < 1e-9);
    assert!((utility_score("TASK: migrate the schema") - 0.9).abs() < 1e-9);
    assert!((utility_score("hi") - 0.15).abs() < 1e-9);
    assert!((utility_score("thanks!") - 0.15).abs() < 1e-9);
    assert!((utility_score("the service listens on port 8080") - 0.5).abs() < 1e-9);
}

#[test]
fn confidence_provenance_and_speculation() {
    assert!((confidence_score(Some(SourceType::UserStated), "my name is Jo") - 0.9).abs() < 1e-9);
    assert!((confidence_score(Some(SourceType::ThirdParty), "docs say x") - 0.7).abs() < 1e-9);
    assert!((confidence_score(Some(SourceType::AgentInferred), "likely x") - 0.6).abs() < 1e-9);
    assert!((confidence_score(None, "plain note") - 0.5).abs() < 1e-9);
    assert!(
        (confidence_score(Some(SourceType::UserStated), "maybe I like tea") - 0.3).abs() < 1e-9
    );
}

#[test]
fn exact_duplicate_detection_ignores_case_and_punctuation() {
    let stored = ["User prefers dark mode"];
    assert!(is_exact_duplicate(
        "user prefers dark mode!",
        &stored.map(String::from)
    ));
    assert!(is_exact_duplicate(
        "  User   prefers   dark mode  ",
        &stored.map(String::from)
    ));
    assert!(is_exact_duplicate(
        "User prefers dark mode",
        &stored.map(String::from)
    ));
}

#[test]
fn exact_duplicate_detection_rejects_near_matches_and_blanks() {
    let stored = [String::from("User prefers dark mode")];
    assert!(!is_exact_duplicate("User prefers light mode", &stored));
    assert!(!is_exact_duplicate(
        "User prefers dark mode in the terminal",
        &stored
    ));
    // An empty candidate has no token set, so it is not a duplicate of anything.
    assert!(!is_exact_duplicate("", &stored));
    assert!(!is_exact_duplicate("!!!", &stored));
    // No neighbors means nothing to duplicate.
    assert!(!is_exact_duplicate("User prefers dark mode", &[]));
}

#[test]
fn word_order_makes_a_different_fact() {
    // A token-set test would call these identical, and would refuse to store a
    // real second fact. Only equal normalized text counts as a duplicate.
    let stored = ["Page 1 Item 2"];
    assert!(!is_exact_duplicate(
        "Page 2 Item 1",
        &stored.map(String::from)
    ));
    assert!(!is_exact_duplicate(
        "Tom walks with Alice",
        &[String::from("Alice walks with Tom")]
    ));
    assert!(is_exact_duplicate(
        "page 1 item 2",
        &stored.map(String::from)
    ));
}

#[test]
fn an_exact_duplicate_and_an_empty_candidate_both_score_zero_novelty() {
    // This equality is why the two signals are separate functions: novelty
    // cannot distinguish "identical" from "no text to compare".
    let dup = novelty_jaccard("the cat sat", &[String::from("the cat sat")]);
    let empty = novelty_jaccard("", &[String::from("the cat sat")]);
    assert!((dup - 0.0).abs() < 1e-9);
    assert!((empty - 0.0).abs() < 1e-9);
    assert!(is_exact_duplicate(
        "the cat sat",
        &[String::from("the cat sat")]
    ));
    assert!(!is_exact_duplicate("", &[String::from("the cat sat")]));
}

#[test]
fn novelty_proxy_edges() {
    assert!((novelty_jaccard("some new fact", &[]) - 1.0).abs() < 1e-9);
    assert!((novelty_jaccard("", &[String::from("anything")]) - 0.0).abs() < 1e-9);
    let dup = novelty_jaccard(
        "the cat sat on the mat",
        &[String::from("the cat sat on the mat")],
    );
    assert!((dup - 0.0).abs() < 1e-9);
    let partial = novelty_jaccard(
        "the cat sat on the mat",
        &[String::from("the cat sat elsewhere entirely")],
    );
    assert!(partial > 0.0 && partial < 1.0);
}

#[test]
fn clamp01_guards_both_ends_and_both_kinds_of_non_finite() {
    // Above one, below zero, and in range: `f64::clamp` handles all three, so
    // the value passes through unchanged when it is already inside the range.
    assert!((clamp01(1.5) - 1.0).abs() < 1e-9);
    assert!((clamp01(-0.25) - 0.0).abs() < 1e-9);
    assert!((clamp01(0.5) - 0.5).abs() < 1e-9);
    assert!((clamp01(0.0) - 0.0).abs() < 1e-9);
    assert!((clamp01(1.0) - 1.0).abs() < 1e-9);
    // An infinite signal is not "very high": it is not a number, so it fails
    // soft at 0.0 the way NaN does, and cannot clamp up to 1.0.
    assert!((clamp01(f64::INFINITY) - 0.0).abs() < 1e-9);
    assert!((clamp01(f64::NEG_INFINITY) - 0.0).abs() < 1e-9);
    assert!((clamp01(f64::NAN) - 0.0).abs() < 1e-9);
}

#[test]
fn factor_unavailable_pins_the_code_and_the_message_shape() {
    let err = factor_unavailable("novelty store");
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
    // The message is the `factor_unavailable: <what>` shape, with the caller's
    // wording kept verbatim after the prefix.
    assert_eq!(err.message, "factor_unavailable: novelty store");
    // The code is also readable from the prefix, so a caller logging only the
    // rendered error still sees which signal failed.
    let rendered = err.to_string();
    assert!(
        rendered.starts_with("factor_unavailable: "),
        "rendered error lost the code prefix: {rendered}"
    );
}

/// The one-byte difference between these two is the whole length gate: the
/// gate reads `text.trim().len()`, so the padding must not change what counts.
const CHATTER_LIMIT: usize = 24;

#[test]
fn utility_chatter_one_byte_below_the_length_limit_scores_low() {
    // The padding makes the trimmed length exactly one under the limit, so the
    // chatter word decides. Without a length gate a long note that merely says
    // "ok" would be demoted to the same score as a bare greeting.
    let text = format!("ok {}", "x".repeat(CHATTER_LIMIT - 4));
    assert_eq!(text.trim().len(), CHATTER_LIMIT - 1);
    assert!(
        (utility_score(&text) - 0.15).abs() < 1e-9,
        "chatter under the limit scored wrong"
    );
}

#[test]
fn utility_chatter_exactly_at_the_length_limit_is_neutral() {
    // The gate is `< 24`, not `<= 24`, so this exact length falls through to the
    // neutral default. The padding keeps the chatter word identical on both
    // sides of the boundary, so only the length differs.
    let text = format!("ok {}", "x".repeat(CHATTER_LIMIT - 3));
    assert_eq!(text.trim().len(), CHATTER_LIMIT);
    assert!(
        (utility_score(&text) - 0.5).abs() < 1e-9,
        "chatter at the limit scored wrong"
    );
}

#[test]
fn confidence_cap_applies_to_every_provenance_equally() {
    // `min` is what makes the cap flat: a high 0.9 and a low 0.5 both land on
    // 0.3, so speculation costs a strongly sourced memory the same as a weak
    // one. Pinning both ends stops a later `max` from quietly lifting it.
    let high = confidence_score(Some(SourceType::UserStated), "maybe I prefer tea");
    let mid = confidence_score(Some(SourceType::AgentInferred), "unsure about the index");
    let low = confidence_score(None, "perhaps the deploy window moves");
    assert!(
        (high - 0.3).abs() < 1e-9,
        "speculative UserStated: got {high}"
    );
    assert!(
        (mid - 0.3).abs() < 1e-9,
        "speculative AgentInferred: got {mid}"
    );
    assert!(
        (low - 0.3).abs() < 1e-9,
        "speculative unknown source: got {low}"
    );
}

#[test]
fn weighted_score_clamps_a_negative_sum_up_to_zero() {
    // The low end of the clamp is reachable when a policy carries a negative
    // weight. `0.0` is the fail-soft choice: a negative score is not a ranking,
    // and letting it through would make every downstream comparison against a
    // threshold meaningless.
    let s = weighted_score([-0.2; 4], [1.0; 4]);
    assert!((s - 0.0).abs() < 1e-9, "negative weighted sum: got {s}");
    // And a zero sum stays zero, so the clamp does not disturb an even split.
    let even = weighted_score([0.25; 4], [0.5; 4]);
    assert!((even - 0.5).abs() < 1e-9);
}

#[test]
fn weighted_score_clamps() {
    let s = weighted_score([0.25; 4], [1.0, 1.0, 1.0, 1.0]);
    assert!((s - 1.0).abs() < 1e-9);
    assert!((clamp01(f64::NAN) - 0.0).abs() < 1e-9);
    assert!((clamp01(2.0) - 1.0).abs() < 1e-9);
    assert_eq!(factor_unavailable("x").code, ErrorCode::FactorUnavailable);
    assert!(
        factor_unavailable("novelty store")
            .to_string()
            .contains("factor_unavailable")
    );
}
