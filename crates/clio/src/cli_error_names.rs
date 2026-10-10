// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for translating the input names a library error
//! message uses into the flag names a CLI caller typed, so a failure read at a
//! shell names something the reader can act on.
//!
//! ## Owns
//! - The whole-message rewrites that cannot be expressed as a subject substitution
//! - The internal-name to flag-name vocabulary, keyed on the message subject
//! - The leading tool-name strip, applied only after a subject has been rewritten
//!
//! ## Does not own
//! - The error type, its exit codes, or its `message`/`hint`/`help` split (see
//!   `cli_error`)
//! - The flag names themselves, which are owned by the usage registry (see
//!   `cli_help_usage`)
//! - Credential masking, which `cli_error` applies before this runs
//!
//! ## Boundary
//!
//! Changes to the CLI spelling of a library input name should normally require
//! changes here. Adding a check to a library should not.
//!
//! Keep this module focused on one vocabulary and the rewriting that uses it.

/// Messages rewritten whole, because a subject substitution would read badly.
///
/// A subject substitution is the right tool when the library message is one
/// sentence whose subject is the field being refused, because the sentence keeps
/// its own wording and its own bound. It is the wrong tool when the message is
/// short enough that the substitution becomes the grammar: `item_id is required`
/// reads well as "the ID argument is required", but "at least one target
/// `item_id` is required" does not.
const WHOLE_MESSAGES: &[(&str, &str)] = &[
    (
        "item_id is required",
        "the ID argument is required, and the one given was empty",
    ),
    (
        "at least one target item_id is required",
        "every clean target needs a non-empty item ID",
    ),
    (
        "item_id is required for every clean target",
        "every clean target needs a non-empty item ID",
    ),
    (
        "retrieve entity_summary requires include_entities (an entity summary lists entity names)",
        "--entity-summary lists entity names, so it cannot be combined with --no-entities",
    ),
];

/// Internal field names and the CLI spelling that carries them.
///
/// Only ever applied to the first word of a message, which is where these
/// messages put the input they are refusing. A whole-message substitution looked
/// tidier and was wrong: `bank` appears inside "another bank must not resolve the
/// record" and `offset` inside "an explicit UTC offset", so a message about the
/// clock came out saying "an explicit UTC `--offset`". Restricting the rule to the
/// subject cannot collide with prose, because prose is not in the subject slot.
const INPUT_NAMES: &[(&str, &str)] = &[
    ("min_scores.semantic", "--min-semantic"),
    ("min_scores.keyword", "--min-keyword"),
    ("min_scores.reranker", "--min-reranker"),
    ("min_scores.final", "--min-final"),
    (
        "min_scores",
        "the --min-semantic, --min-keyword, --min-reranker or --min-final flag",
    ),
    ("query_anchor", "--query-anchor"),
    ("entity_summary", "--entity-summary"),
    ("include_discarded", "--include-discarded"),
    ("epistemic_kind", "--epistemic-kind"),
    ("source_type", "--source-type"),
    ("source_text", "--source-text"),
    ("evidence_ref", "--evidence-ref"),
    ("item_id", "the ID argument"),
    ("limit", "--limit"),
    ("bank", "--bank"),
    ("actor", "--actor"),
];

/// Leading tool names stripped from a message the CLI is about to rewrite.
///
/// A library message opens with the tool that produced it, which is the right
/// prefix for an MCP caller and a word nobody at a shell typed. The prefix is
/// dropped only once the subject has been translated, so a message that names no
/// CLI input keeps its prefix rather than losing information.
///
/// The list is checked against the command binding table by the tests, so a tool
/// added without a row here is a test failure rather than a silent leftover.
const TOOL_PREFIXES: &[&str] = &[
    "retrieve",
    "store",
    "admit_preview",
    "consolidate",
    "summarize",
    "update",
    "invalidate",
    "discard",
    "correct",
    "inspect",
    "validate",
    "batch",
    "export",
    "import",
    "erase_request",
];

/// Rewrite a library message's subject into the CLI's vocabulary.
///
/// Returns `message` unchanged when it names no input the CLI spells differently,
/// which is the common case: most errors are about the store, not about a flag.
pub(super) fn to_cli_spelling(message: &str) -> String {
    if let Some((_, cli)) = WHOLE_MESSAGES.iter().find(|(from, _)| *from == message) {
        return (*cli).to_owned();
    }
    let subject_at = tool_prefix_end(message);
    let Some((subject, rest)) = split_subject(&message[subject_at..]) else {
        return message.to_owned();
    };
    let Some((_, cli)) = INPUT_NAMES.iter().find(|(name, _)| *name == subject) else {
        return message.to_owned();
    };
    format!("{cli}{rest}")
}

/// Where the subject begins, having skipped a leading tool name.
fn tool_prefix_end(message: &str) -> usize {
    TOOL_PREFIXES
        .iter()
        .find_map(|tool| {
            message
                .strip_prefix(tool)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(|_| tool.len() + 1)
        })
        .unwrap_or(0)
}

/// Split a message's first word from the rest, ignoring trailing punctuation.
///
/// `min_scores: {e}` has to match the `min_scores` row, so the colon belongs to
/// the rest of the message rather than to the name.
fn split_subject(text: &str) -> Option<(&str, &str)> {
    let end = text
        .find(|character: char| !(character.is_alphanumeric() || "_.-".contains(character)))
        .unwrap_or(text.len());
    (end > 0).then(|| (&text[..end], &text[end..]))
}

#[cfg(test)]
#[path = "cli_error_names_tests.rs"]
mod tests;
