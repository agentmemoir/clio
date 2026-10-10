// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the binary dispatch edge paths for the read surface:
//! subcommand `--help`, explicit `--output`/`-o` on an unknown command, a
//! leading global value flag with no value, and the top-level output-mode help.
//!
//! ## Owns
//! - Dispatch and exit-code tests for the read verbs and unknown commands
//! - `parse_flags` alias tests and the output-mode help text
//!
//! ## Does not own
//! - The CLI implementation under test (`main`)
//! - The core dispatch tests (`main_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_flags_test_support::flags_of;

#[test]
fn unknown_command_honors_explicit_output_flag() {
    // The explicit `--output` is scanned from the raw tokens even though the
    // command itself is unknown; the run still exits 2 as a usage error.
    assert_eq!(
        run(vec!["recal".into(), "--output".into(), "text".into()]),
        ExitCode::from(2)
    );
    assert_eq!(
        run(vec!["--output=json".into(), "recal".into()]),
        ExitCode::from(2)
    );
    // The `-o` alias is scanned the same way, before or after the verb.
    assert_eq!(
        run(vec!["recal".into(), "-o".into(), "text".into()]),
        ExitCode::from(2)
    );
    assert_eq!(
        run(vec!["-o=json".into(), "recal".into()]),
        ExitCode::from(2)
    );
}

#[test]
fn read_command_missing_leading_global_value_exits_two() {
    // A leading value flag with no value must not swallow the next flag.
    assert_eq!(
        run(vec![
            "--output".into(),
            "--db".into(),
            "sqlite::memory:".into(),
            "recall".into(),
            "q".into()
        ]),
        ExitCode::from(2)
    );
    assert_eq!(
        run(vec![
            "-o".into(),
            "--db".into(),
            "sqlite::memory:".into(),
            "recall".into(),
            "q".into()
        ]),
        ExitCode::from(2)
    );
}

#[test]
fn parse_flags_recognizes_the_short_output_alias() {
    let flags = flags_of(&["-o", "json"]);
    assert_eq!(flags.get("output").map(String::as_str), Some("json"));

    let flags = flags_of(&["-o=text", "--db", "x"]);
    assert_eq!(flags.get("output").map(String::as_str), Some("text"));
    assert_eq!(flags.get("db").map(String::as_str), Some("x"));

    // A value-less `-o` is recorded as a bare flag, exactly like `--output`.
    let flags = flags_of(&["-o", "--db", "x"]);
    assert_eq!(flags.get("output").map(String::as_str), Some("true"));
    assert_eq!(flags.get("db").map(String::as_str), Some("x"));
}

/// Every other single-dash token is an unknown flag: it fails closed with a
/// usage error rather than being dropped.
#[test]
fn an_unknown_short_flag_is_a_usage_error() {
    let error = crate::cli_top_level::parse_flags(
        ["-x".to_owned()],
        crate::cli_flags_test_support::RESERVED_FLAGS,
    )
    .expect_err("an unknown short flag must be a usage error");
    assert_eq!(error.code(), "usage");
}

#[test]
fn help_documents_the_output_mode_rule_and_short_alias() {
    // The catalog wraps to a column, so a phrase can straddle a line break. The
    // text is compared with whitespace collapsed, which is the only way to assert
    // a sentence about a wrapped block.
    let text = crate::cli_top_level::help_text()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in [
        "Global flags: --db --backend --bank --actor --output (-o).",
        "an explicit `--output`/`-o` always wins;",
        "text on a terminal (TTY) and JSON when piped.",
        "`-o json`, `-o=json`.",
        "the shell shows that as 141, and it is not one of these exit codes",
    ] {
        assert!(text.contains(phrase), "help must say {phrase:?}:\n{text}");
    }
}

#[test]
fn read_subcommand_help_exits_zero() {
    for verb in [
        "recall",
        "get",
        "show",
        "inspect",
        "stats",
        "associations",
        "compose-context",
    ] {
        assert_eq!(
            run(vec![verb.into(), "--help".into()]),
            ExitCode::SUCCESS,
            "{verb}"
        );
    }
}

#[test]
fn group_command_help_exits_zero() {
    // Group help (no subcommand) and subcommand help both exit 0 without
    // opening a database.
    for cmd in [
        "task",
        "failure",
        "memtree",
        "history",
        "triple",
        "belief",
        "graph",
        "persona",
        "intent",
        "maintenance",
        "audit",
    ] {
        assert_eq!(
            run(vec![cmd.into(), "--help".into()]),
            ExitCode::SUCCESS,
            "{cmd}"
        );
    }
    assert_eq!(
        run(vec!["task".into(), "get".into(), "--help".into()]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run(vec!["history".into(), "temporal".into(), "--help".into()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn group_command_missing_subcommand_exits_two() {
    assert_eq!(run(vec!["task".into()]), ExitCode::from(2));
    assert_eq!(run(vec!["task".into(), "bogus".into()]), ExitCode::from(2));
}

#[test]
fn top_level_help_binds_every_read_command_tool() {
    // Every command→tool binding must appear in the top-level help with its
    // `-> <tool>` arrow, so the human help never drifts from the catalog.
    // Two tools sharing one line are joined with ` / `.
    let text = crate::cli_top_level::help_text();
    for (command, tool) in crate::cli_help::command_bindings() {
        let bound = text.contains(&format!("-> {tool}")) || text.contains(&format!(" / {tool}"));
        assert!(bound, "help is missing `-> {tool}` for command `{command}`");
        assert!(
            text.contains(command),
            "help is missing command `{command}`"
        );
    }
}
