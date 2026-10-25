// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that exit 75 carries one meaning on every surface that
//! names it: the canonical sentence lives once, and the records that describe
//! the code carry that sentence word for word instead of promising a retry the
//! command itself does not clear.
//!
//! ## Owns
//! - The canonical sentence's own content, clause by clause
//! - The help record's summary and the restore record's summary carrying it
//! - The absence of the blanket "retryable" claim the summaries used to make
//!
//! ## Does not own
//! - The condition-to-code table (`backup_restore_exit_tests`)
//! - The restore's text and JSON views (`backup_restore_report_reindex_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every expectation here is
//! a literal written in this file; none of it comes from the host.
//!
//! Keep this module focused on the one meaning of the temporary code.

use super::EXIT_TEMPORARY_MEANING;
use crate::cli_command_registry_test_support::record;

/// What the canonical sentence names, as the operator must read it: the code is
/// not a clean success, it lost and invalidated nothing, and the retry answer
/// belongs to the command's own message rather than to the code.
const REQUIRED_CLAUSES: &[&str] = &[
    "could not report a clean success",
    "nothing was lost or invalid",
    "the command's own message says whether a retry helps",
];

/// The canonical sentence names all three facts in one sentence, so no surface
/// can word 75 as a clean failure or as a retry that always helps.
#[test]
fn the_canonical_sentence_names_all_three_facts() {
    let sentence = EXIT_TEMPORARY_MEANING;

    assert!(sentence.starts_with("75 means "), "{sentence}");
    for clause in REQUIRED_CLAUSES {
        assert!(sentence.contains(clause), "`{clause}` in: {sentence}");
    }
    assert_eq!(
        sentence.matches(". ").count(),
        0,
        "one sentence, so its clauses cannot be quoted apart: {sentence}"
    );
}

/// The retry answer is split by case, in the same sentence: a storage lock
/// conflict clears on a retry, and a restore that left a derived difference
/// does not, because re-running it reloads the same archive.
#[test]
fn the_canonical_sentence_says_which_condition_a_retry_clears() {
    let sentence = EXIT_TEMPORARY_MEANING;

    let (retryable, restore) = sentence
        .split_once(": ")
        .unwrap_or_else(|| panic!("the cases follow the colon: {sentence}"));
    assert!(
        retryable.contains("the command's own message says whether a retry helps"),
        "the retry answer belongs to the message: {sentence}"
    );
    assert!(
        restore.starts_with("a storage lock conflict is cleared by retrying"),
        "the retryable case is named first: {sentence}"
    );
    assert!(
        restore.ends_with("a restore that left a derived difference is not"),
        "the restore case is the one a retry does not clear: {sentence}"
    );
}

/// Every surface that names 75 carries the canonical sentence word for word,
/// so the help screen and the command's own summary cannot disagree.
#[test]
fn every_surface_that_names_the_code_repeats_the_canonical_sentence() {
    for name in ["help", "backup restore"] {
        let summary = record(name).summary;
        assert!(
            summary.contains(EXIT_TEMPORARY_MEANING),
            "`{name}` must carry the canonical sentence: {summary}"
        );
    }
}

/// Neither summary claims the code is retryable on its own: the old wording
/// promised a retry that a degraded restore contradicts.
#[test]
fn no_surface_claims_the_code_is_retryable_without_naming_the_case() {
    for name in ["help", "backup restore"] {
        let summary = record(name).summary;
        assert!(
            !summary.contains("retryable"),
            "`{name}` claims a blanket retry: {summary}"
        );
        assert!(
            !summary.contains("a retry confirms"),
            "`{name}` says a retry confirms the count: {summary}"
        );
    }
}
