// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the pinned mission pattern
//! subset: parse errors, matching semantics, and the step-budget bound.
//!
//! ## Owns
//! - Subset grammar rejection tests (groups, escapes, braces, nested quantifiers)
//! - Case-insensitive matching, classes, quantifiers, and alternation
//! - The budget bound that treats exhausted evaluation as a non-match
//!
//! ## Does not own
//! - Mission schema validation (see `retention_mission_tests`)
//! - Admission influence of matched entries (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the grammar or bound should normally require changes here.
//! Changes to profile semantics should not.
//!
//! Keep this module focused on the pinned pattern subset.

use crate::mission_match::{MissionMatchResult, compile, is_match};

fn matches(pattern: &str, text: &str) -> MissionMatchResult {
    let pattern = compile(pattern).expect("valid pattern");
    is_match(&pattern, &text.to_lowercase().chars().collect::<Vec<_>>())
}

#[test]
fn subset_grammar_accepts_documented_syntax() {
    for pattern in [
        "tests passed",
        "pr created|issue created",
        "[0-9]+ error[sS]?",
        "[a-z]*-build",
        "heart?beat*",
    ] {
        compile(pattern).unwrap_or_else(|e| panic!("{pattern} must compile: {e}"));
    }
}

#[test]
fn subset_grammar_rejects_everything_else() {
    for pattern in [
        "(a)+",   // groups
        "(?:ab)", // non-capturing groups
        "a{2,3}", // bounded repetition braces
        r"\d+",   // escapes
        "a**",    // nested quantifiers
        "a?+",    // nested quantifiers
        "a|",     // empty alternation branch
        "|a",     // empty first branch
        "[a-",    // unterminated class
        "[z-a]",  // inverted range
    ] {
        assert!(compile(pattern).is_err(), "`{pattern}` must be rejected");
    }
    let long = "a".repeat(201);
    assert!(
        compile(&long).is_err(),
        "patterns over the cap are rejected"
    );
}

#[test]
fn matching_is_case_insensitive_and_unanchored() {
    assert_eq!(
        matches("tests passed", "All Tests Passed"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("pr created", "the PR created for x"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("pr created", "pull request opened"),
        MissionMatchResult::NoMatch
    );
}

#[test]
fn classes_quantifiers_and_alternation_match() {
    assert_eq!(
        matches("issue [0-9]+", "Issue 412 fixed"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("issue [0-9]+", "Issue abc"),
        MissionMatchResult::NoMatch
    );
    assert_eq!(matches("[^0-9]+", "abc"), MissionMatchResult::Matched);
    assert_eq!(matches("[^0-9]+", "42"), MissionMatchResult::NoMatch);
    assert_eq!(
        matches("pr created|issue created", "an issue created earlier"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("heart?beats?", "heartbeat"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("heart?beats?", "hearbeat"),
        MissionMatchResult::Matched
    );
    assert_eq!(
        matches("heart?beats?", "hearts"),
        MissionMatchResult::NoMatch
    );
    assert_eq!(matches("a+", "caaag"), MissionMatchResult::Matched);
}

#[test]
fn budget_bounded_patterns_exhaust_instead_of_stalling() {
    // Ten stacked star terms over a long candidate text is polynomial at best
    // on a backtracker; the step budget must cut it off deterministically.
    let pattern = "a*".repeat(10) + "b";
    let text = "a".repeat(120);
    assert_eq!(
        matches(&pattern, &text),
        MissionMatchResult::BudgetExhausted
    );
}

/// Greedy backtracking: `a*ab` on "aaab" first tries three repetitions, must
/// back off to two before the `a` can match. Locks the position-fallback
/// behavior of the repetition matcher.
#[test]
fn greedy_repetition_backtracks_to_shorter_counts() {
    assert_eq!(matches("a*ab", "aaab"), MissionMatchResult::Matched);
    assert_eq!(matches("a*ab", "ab"), MissionMatchResult::Matched);
    assert_eq!(matches("a*ab", "aXb"), MissionMatchResult::NoMatch);
    // Plus needs at least one repetition before the tail may match.
    assert_eq!(matches("a+ab", "ab"), MissionMatchResult::NoMatch);
    assert_eq!(matches("a+ab", "aab"), MissionMatchResult::Matched);
}
