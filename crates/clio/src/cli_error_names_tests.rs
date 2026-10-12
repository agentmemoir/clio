// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI spelling of a library error message: that the
//! internal field name in the subject is replaced by the flag the caller typed,
//! and that nothing else in the sentence is touched.
//!
//! ## Owns
//! - Every reported case from the argument-error sweep
//! - The negative cases that keep the rewrite from reaching into prose
//! - The agreement between the vocabulary and the real tool names
//!
//! ## Does not own
//! - The error type and its exit codes (`cli_error_tests`)
//! - The flag names themselves (`cli_help_usage_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{INPUT_NAMES, TOOL_PREFIXES, WHOLE_MESSAGES, to_cli_spelling};

/// Every reported case: the library names its field, the caller is told the flag.
#[test]
fn the_reported_internal_names_become_cli_flags() {
    let cases = [
        (
            "retrieve limit must be positive",
            "--limit must be positive",
        ),
        (
            "retrieve min_scores.semantic must be <= 1",
            "--min-semantic must be <= 1",
        ),
        (
            "retrieve min_scores.keyword must be >= 0",
            "--min-keyword must be >= 0",
        ),
        (
            "retrieve min_scores.reranker must be <= 1",
            "--min-reranker must be <= 1",
        ),
        (
            "retrieve min_scores.final must be <= 1000000",
            "--min-final must be <= 1000000",
        ),
        (
            "domain must be semantic|episodic|task|failure|temporal|persona, got `bogus`",
            "--domains must be semantic|episodic|task|failure|temporal|persona, got `bogus`",
        ),
        (
            "query_anchor must be an RFC 3339 timestamp with an explicit UTC offset",
            "--query-anchor must be an RFC 3339 timestamp with an explicit UTC offset",
        ),
        ("bank is required", "--bank is required"),
        (
            "actor must be agent|harness|user|system, got `x`",
            "--actor must be agent|harness|user|system, got `x`",
        ),
        ("inspect limit must be positive", "--limit must be positive"),
    ];
    for (library, cli) in cases {
        assert_eq!(
            to_cli_spelling(library),
            cli,
            "`{library}` should reach a shell as `{cli}`"
        );
    }
}

/// The reported cases whose sentence is short enough that a subject substitution
/// would become the grammar, so the whole message is rewritten.
#[test]
fn the_reported_short_messages_are_rewritten_whole() {
    for (library, cli) in WHOLE_MESSAGES {
        assert_eq!(
            to_cli_spelling(library),
            *cli,
            "`{library}` should reach a shell as `{cli}`"
        );
    }
}

/// Prose is never rewritten. Every case here is a real library message in which
/// the vocabulary word appears mid-sentence; rewriting any of them is how a
/// message about a clock came to say "an explicit UTC `--offset`".
#[test]
fn a_field_name_inside_prose_is_left_alone() {
    let messages = [
        "retrieve requires a non-empty query",
        "lexical query must be non-empty",
        "a revision cannot move a unit to another bank",
        "another bank must not resolve the record",
        "hygiene log limit must be in 1..=200, got 5",
        "an emitted context must not be empty",
        "dense_search limit must be positive",
    ];
    for message in messages {
        assert_eq!(
            to_cli_spelling(message),
            message,
            "`{message}` names no input in the subject slot, so nothing changes"
        );
    }
}

/// A message about the store keeps its own vocabulary: nothing here is a flag the
/// caller can type, so nothing is translated.
#[test]
fn a_store_failure_is_passed_through_untouched() {
    let message = "no retention store configured; pass `--profiles FILE`";
    assert_eq!(to_cli_spelling(message), message);
}

/// Every flag a vocabulary row points at is one the CLI really accepts.
///
/// The invariant is about the flags, not about the shape of the replacement: most
/// rows name a flag, and `item_id` names the ID argument, because an id is a
/// positional here and there is no `--item-id`. What would be wrong is a row
/// sending a reader to a flag no command takes.
#[test]
fn every_flag_a_vocabulary_row_names_is_one_a_command_accepts() {
    let known = all_flag_names();
    for (internal, cli) in INPUT_NAMES {
        let mut rest: &str = cli;
        let mut named: Vec<String> = Vec::new();
        while let Some(at) = rest.find("--") {
            let after = &rest[at + 2..];
            let end = after
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(after.len());
            named.push(format!("--{}", &after[..end]));
            rest = &after[end..];
        }
        for flag in &named {
            assert!(
                known.iter().any(|name| name == flag),
                "`{internal}` translates to `{cli}`, which names {flag}: that is not a \
                 flag any command accepts, so it would send the reader to a flag that \
                 does not exist"
            );
        }
    }
}

/// The tool prefixes stripped from a message must be real tools, or a message
/// about something else would lose its first word.
#[test]
fn every_tool_prefix_is_a_real_tool() {
    let tools: Vec<&str> = crate::cli_help::command_bindings()
        .iter()
        .map(|(_, tool)| *tool)
        .collect();
    for prefix in TOOL_PREFIXES {
        assert!(
            tools.contains(prefix),
            "`{prefix}` is stripped as a tool name but binds no CLI command, so a \
             message that happens to start with that word would lose it"
        );
    }
}

/// Every flag any command's usage line names, sorted so the failure message is
/// stable across runs.
fn all_flag_names() -> Vec<String> {
    let mut names: Vec<String> = crate::command_catalog::catalog_commands()
        .filter_map(crate::cli_help_usage::usage_line)
        .flat_map(|usage| crate::help_flags_test_support::flags_in(&usage))
        .collect();
    names.sort();
    names.dedup();
    names
}
