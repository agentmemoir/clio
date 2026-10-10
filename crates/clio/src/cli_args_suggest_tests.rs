// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the suggestion matcher: that a typo of a command or a flag
//! is offered the closest real name, and that an unrelated word is offered none.
//!
//! ## Owns
//! - The Jaro similarity cases taken from `clap`'s own published behaviour
//! - The two cases that only exist because of a real report: a transposed short
//!   verb, and a long unrelated word that similarity alone matched
//!
//! ## Does not own
//! - The parser that calls `suggest` (`cli_args`)
//! - The command list the matcher is given (`cli_help`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::suggest;
use super::{SUGGEST_THRESHOLD, jaro, ratio};

/// The cases `clap` publishes for its own `did you mean` output. Matching them
/// pins this matcher to an established behaviour rather than to whatever it
/// happens to do today, and the source is `clap`'s `suggestions` module, which
/// scores with Jaro similarity above 0.7.
///
/// Every case also carries the extra rule this matcher adds: a shared first
/// letter. `clap` has no such rule, and these candidates all share one anyway.
#[test]
fn a_mistyped_word_finds_the_word_it_was_meant_to_be() {
    let values = ["test", "possible", "values"];
    assert_eq!(suggest("tst", &values), Some("test"), "one letter dropped");
}

/// Two candidates score exactly alike, so the tie-break decides: the earlier
/// candidate wins, so the answer is a function of the candidate order and never of
/// how two equal comparisons happened to land.
#[test]
fn two_equally_close_candidates_keep_the_earlier_one() {
    let values = ["test", "temp", "possible", "values"];
    assert_eq!(suggest("te", &values), Some("test"));
}

/// A long prefix of shared letters still finds the right tail.
#[test]
fn a_mistyped_long_word_finds_both_candidates_that_match() {
    let values = [
        "test",
        "possible",
        "values",
        "alignmentStart",
        "alignmentScore",
    ];
    assert_eq!(
        suggest("alignmentScorr", &values),
        Some("alignmentScore"),
        "the better of two long near-identical candidates"
    );
}

/// A word with nothing in common with any candidate gets no suggestion, which is
/// what leaves the caller with the pointer to the command's own help.
#[test]
fn an_unrelated_word_is_offered_nothing() {
    let values = ["test", "possible", "values"];
    assert_eq!(suggest("hahaahahah", &values), None);
}

/// The reported case: `clio recod` is three edits on six characters and an edit
/// distance refuses to call it `recall`.
#[test]
fn a_transposed_short_verb_is_suggested() {
    let verbs = ["recall", "remember", "get", "inspect", "consolidate"];
    assert_eq!(
        suggest("recod", &verbs),
        Some("recall"),
        "a dropped letter in a short verb must still be suggested"
    );
}

/// The reported case: `clio rember` already worked and must keep working.
#[test]
fn a_letter_swapped_verb_is_suggested() {
    let verbs = ["recall", "remember", "get", "inspect"];
    assert_eq!(suggest("rember", &verbs), Some("remember"));
}

/// The case that made similarity alone unusable: `frobnicate` scores 0.715
/// against `consolidate` — six characters shared in order — and suggesting
/// `clio consolidate` for `clio frobnicate` is worse than suggesting nothing.
#[test]
fn a_word_that_shares_letters_but_not_a_first_letter_is_offered_nothing() {
    let verbs = [
        "version",
        "health",
        "recall",
        "consolidate",
        "compose-context",
        "canonical",
        "shared",
    ];
    assert_eq!(
        suggest("frobnicate", &verbs),
        None,
        "similarity without a shared first letter is not a typo"
    );
}

/// The same rule on the flag side, where a stray single-dash token used to be
/// left with no hint at all.
#[test]
fn a_single_dash_token_is_offered_no_flag() {
    let flags = ["limit", "domains", "as-of", "output", "yes", "help"];
    assert_eq!(suggest("x", &flags), None);
}

/// The flag case the matcher exists for: one extra letter on a flag name.
#[test]
fn an_extra_letter_in_a_flag_name_is_suggested() {
    let flags = [
        "limit",
        "domains",
        "as-of",
        "query-anchor",
        "output",
        "help",
    ];
    assert_eq!(suggest("limitt", &flags), Some("limit"));
}

/// A mistyped enum value gets the same treatment as a mistyped flag, which is
/// what makes `--category` name a value the caller can paste.
#[test]
fn a_mistyped_enum_value_finds_its_own_value() {
    let categories = [
        "task_spec",
        "schema",
        "tool_config",
        "output_constraint",
        "persona",
    ];
    assert_eq!(suggest("person", &categories), Some("persona"));
}

/// An empty candidate list is not a panic and not a suggestion.
#[test]
fn an_empty_candidate_list_is_offered_nothing() {
    assert_eq!(suggest("recall", &[]), None);
}

/// The cut-off itself, pinned rather than left to the implementation's arithmetic.
///
/// It is the value `clap` uses for its own `did you mean` output, included at the
/// boundary because `recod` against `recall` scores exactly on it.
#[test]
fn the_cut_off_is_the_documented_value() {
    assert!(
        (SUGGEST_THRESHOLD - 0.7).abs() < f64::EPSILON,
        "{SUGGEST_THRESHOLD}"
    );
}

/// The two overlap ratios the measure averages.
#[test]
fn a_ratio_is_a_part_over_a_whole() {
    assert!((ratio(1, 2) - 0.5).abs() < 1e-12);
    assert!((ratio(3, 3) - 1.0).abs() < 1e-12);
}

/// Two strings with nothing in common score zero, and two that are equal score
/// one, which are the two ends the matcher relies on.
#[test]
fn the_measure_spans_zero_to_one() {
    assert!(
        (jaro("", "") - 1.0).abs() < 1e-12,
        "two empty strings match"
    );
    assert!(
        (jaro("", "x") - 0.0).abs() < 1e-12,
        "an empty string matches nothing"
    );
    assert!(
        (jaro("abc", "xyz") - 0.0).abs() < 1e-12,
        "nothing shared scores zero"
    );
    assert!(
        (jaro("recall", "recall") - 1.0).abs() < 1e-12,
        "a word matches itself"
    );
}

/// The published textbook pair, pinned to the number every Jaro implementation
/// must produce.
///
/// `martha` against `marhta` is the example the measure's author used, and
/// 0.9444… is its score in `strsim` — the implementation `clap` uses. Pinning it
/// is a stronger check than any ordering invented here, because a wrong
/// transposition count or a wrong window moves this number.
#[test]
fn the_measure_matches_the_published_example() {
    assert!((jaro("martha", "marhta") - 0.944_444_444_444_444_5).abs() < 1e-12);
}
