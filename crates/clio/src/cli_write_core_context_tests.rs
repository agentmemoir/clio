// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the optional bounded source `context` field at the `clio`
//! CLI edge: `--context` decoding for `remember`/`admit`/`correct`, the usage
//! failures for empty and oversized values, the usage lines that document the
//! flag, and the `get` text view.
//!
//! ## Owns
//! - Behavior tests for `--context` decoding, validation, and text rendering
//!
//! ## Does not own
//! - The commands under test (`cli_write_core`, `cli_write_mutate`,
//!   `cli_help_usage`, `cli_read_render`)
//! - The MCP tool schemas that consume the decoded arguments
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_args::parse;
use crate::cli_error::CliError;
use crate::cli_read_render::render_text;
use crate::cli_read_test_support::{state, tokens};
use crate::cli_write_core::CoreWriteGroup;
use crate::cli_write_group::{WriteCall, WriteGroup};
use crate::cli_write_mutate::MutationWriteGroup;
use crate::cli_write_test_support::run_write_state;
use clio_types::CONTEXT_MAX_BYTES;
use serde_json::{Value, json};
use std::process::ExitCode;

/// Parse `args` against `group`'s spec and build the decoded tool call.
fn tool_call(group: &dyn WriteGroup, verb: &str, args: &[&str]) -> (&'static str, Value) {
    let parsed = parse(&group.spec(verb), tokens(args)).expect("flags parse");
    match group.build(verb, &parsed).expect("call builds") {
        WriteCall::Tool { tool, args } => (tool, args),
        WriteCall::Preview { payload, .. } => panic!("expected a tool call, got {payload}"),
    }
}

/// Decode `args` and expect the usage failure (exit 2).
fn usage_error(group: &dyn WriteGroup, verb: &str, args: &[&str]) -> CliError {
    let parsed = parse(&group.spec(verb), tokens(args)).expect("flags parse");
    match group.build(verb, &parsed) {
        Ok(_) => panic!("expected a usage error"),
        Err(error) => error,
    }
}

/// Expect the argument parser itself to refuse `args`.
///
/// An empty value for a value flag is refused by the one rule that covers every
/// flag, so it never reaches the command that would otherwise check it.
fn parse_refusal(group: &dyn WriteGroup, verb: &str, args: &[&str]) -> CliError {
    match parse(&group.spec(verb), tokens(args)) {
        Ok(parsed) => panic!(
            "the parser accepted `{args:?}` as {:?}",
            parsed.positionals()
        ),
        Err(error) => error,
    }
}

#[test]
fn remember_passes_context_unchanged_to_the_store_call() {
    let (tool, args) = tool_call(
        &CoreWriteGroup,
        "remember",
        &["x", "--category", "persona", "--context", "team chat"],
    );
    assert_eq!(tool, "store");
    assert_eq!(args["context"], json!("team chat"), "{args}");
}

#[test]
fn remember_without_context_builds_exactly_the_same_arguments() {
    let (_, args) = tool_call(&CoreWriteGroup, "remember", &["x", "--category", "persona"]);
    assert!(args.get("context").is_none(), "{args}");
    let keys: Vec<&str> = args
        .as_object()
        .expect("args object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["category", "epistemic_kind", "item"]);
}

#[test]
fn remember_rejects_an_empty_context_before_dispatch() {
    let error = parse_refusal(
        &CoreWriteGroup,
        "remember",
        &["x", "--category", "persona", "--context", ""],
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("--context"), "{}", error.message());
    assert!(error.message().contains("empty"), "{}", error.message());

    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &["x", "--category", "persona", "--context", ""],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "no tool call is dispatched: {out}");
    assert!(err.contains("--context"), "{err}");
}

#[test]
fn remember_context_boundary_is_4096_utf8_bytes() {
    let accepted = "a".repeat(CONTEXT_MAX_BYTES);
    let (_, args) = tool_call(
        &CoreWriteGroup,
        "remember",
        &["x", "--category", "persona", "--context", accepted.as_str()],
    );
    assert_eq!(
        args["context"].as_str().map(str::len),
        Some(CONTEXT_MAX_BYTES)
    );

    let oversized = "a".repeat(CONTEXT_MAX_BYTES + 1);
    let error = usage_error(
        &CoreWriteGroup,
        "remember",
        &[
            "x",
            "--category",
            "persona",
            "--context",
            oversized.as_str(),
        ],
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("4096"), "{}", error.message());
    assert!(error.message().contains("4097"), "{}", error.message());
    assert!(
        !error.message().contains(&oversized),
        "the raw value must not be echoed: {}",
        error.message()
    );
}

#[test]
fn admit_mirrors_the_same_context_arguments() {
    let (tool, args) = tool_call(
        &CoreWriteGroup,
        "admit",
        &["x", "--category", "persona", "--context", "team chat"],
    );
    assert_eq!(tool, "admit_preview");
    assert_eq!(args["context"], json!("team chat"), "{args}");

    let error = parse_refusal(
        &CoreWriteGroup,
        "admit",
        &["x", "--category", "persona", "--context", ""],
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("--context"), "{}", error.message());
}

#[test]
fn admit_batch_file_rejects_a_command_level_context() {
    let error = usage_error(
        &CoreWriteGroup,
        "admit",
        &["--file", "ops.json", "--context", "team chat"],
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("--context"), "{}", error.message());
    assert!(error.message().contains("--file"), "{}", error.message());
}

#[test]
fn correct_sends_context_in_the_tool_call() {
    let (tool, args) = tool_call(
        &MutationWriteGroup,
        "correct",
        &[
            "itm-1",
            "new value",
            "--reason",
            "typo",
            "--context",
            "team chat",
            "--bank",
            "demo",
        ],
    );
    assert_eq!(tool, "correct");
    assert_eq!(args["context"], json!("team chat"), "{args}");
    assert_eq!(args["reason"], json!("typo"), "{args}");
}

#[test]
fn correct_without_context_omits_the_key() {
    let (_, args) = tool_call(
        &MutationWriteGroup,
        "correct",
        &["itm-1", "new value", "--reason", "typo"],
    );
    assert!(args.get("context").is_none(), "{args}");
}

#[test]
fn correct_rejects_an_empty_and_an_oversized_context() {
    // The empty value is the parser's one rule; the oversized one is still the
    // command's own bound, which is a different limit.
    let empty = parse_refusal(
        &MutationWriteGroup,
        "correct",
        &["itm-1", "new value", "--reason", "typo", "--context", ""],
    );
    assert_eq!(empty.exit_code(), ExitCode::from(2));
    assert!(empty.message().contains("--context"), "{}", empty.message());

    let oversized = "a".repeat(CONTEXT_MAX_BYTES + 1);
    let error = usage_error(
        &MutationWriteGroup,
        "correct",
        &[
            "itm-1",
            "new value",
            "--reason",
            "typo",
            "--context",
            &oversized,
        ],
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("--context"), "{}", error.message());
}

#[test]
fn discard_does_not_accept_context() {
    let error = parse(
        &MutationWriteGroup.spec("discard"),
        tokens(&["itm-1", "--reason", "r", "--context", "x"]),
    )
    .expect_err("unknown flag");
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );
}

#[test]
fn usage_lines_document_the_context_flag() {
    for verb in ["remember", "admit", "correct"] {
        let usage = crate::cli_help_usage::usage_for(verb).expect("usage line");
        assert!(usage.contains("[--context TEXT]"), "{verb}: {usage}");
    }
    let discard = crate::cli_help_usage::usage_for("discard").expect("usage line");
    assert!(!discard.contains("--context"), "{discard}");
}

/// A user who tags a memory `--context goals` and then recalls "my goals"
/// gets nothing back, because context is never a retrieval signal. The write
/// help has to say so where the flag is offered, or the tag reads as a topic.
/// Both verbs that take the flag on a text write carry the sentence.
#[test]
fn write_usage_lines_say_context_is_not_searchable() {
    for verb in ["remember", "admit"] {
        let usage = crate::cli_help_usage::usage_for(verb).expect("usage line");
        assert!(
            usage.contains("not searchable"),
            "{verb} must tell the user --context is not searchable: {usage}"
        );
        assert!(
            usage.contains("TEXT"),
            "{verb} must point the user at TEXT for anything searchable: {usage}"
        );
    }
}

#[test]
fn get_text_shows_context_when_present() {
    let text = render_text(
        "get",
        &json!({"item_id": "itm-1", "gist": "g", "context": "team chat"}),
        crate::cli_style::Palette::new(false),
    );
    assert!(text.contains("\ncontext: team chat"), "{text}");
}

#[test]
fn get_text_omits_context_when_absent_or_empty() {
    let absent = render_text(
        "get",
        &json!({"item_id": "itm-1", "gist": "g"}),
        crate::cli_style::Palette::new(false),
    );
    assert!(!absent.contains("context"), "{absent}");
    let empty = render_text(
        "get",
        &json!({"item_id": "itm-1", "gist": "g", "context": ""}),
        crate::cli_style::Palette::new(false),
    );
    assert!(!empty.contains("context"), "{empty}");
}
