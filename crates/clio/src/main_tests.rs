// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the core `clio` CLI surface outside the MCP server: the
//! top-level command dispatch, its exit codes, `parse_flags` mechanics, and
//! help-output credential masking.
//!
//! ## Owns
//! - Dispatch and exit-code tests for `version`, `health`, `help`, and `setup`
//! - `parse_flags` pair, bare, and `--flag=value` mechanics tests
//! - Help-output and credential-masking tests
//!
//! ## Does not own
//! - The CLI implementation under test (`main`)
//! - The `mcp` command surface tests (`main_mcp_tests`)
//! - The `clio ops` surface tests (`ops_cli`)
//! - End-to-end process tests over a real binary (`tests/`)
//!
//! ## Boundary
//!
//! Tests here break when the top-level command surface, its flag grammar, or
//! its exit-code contract changes; they never change it.
//!
//! Keep this module focused on the core command surface.

use super::*;
use crate::cli_flags_test_support::flags_of;

#[test]
fn default_args_print_version() {
    assert_eq!(run(Vec::<String>::new()), ExitCode::SUCCESS);
}

#[test]
fn version_aliases() {
    for cmd in ["version", "--version", "-V"] {
        assert_eq!(run(vec![cmd.into()]), ExitCode::SUCCESS);
    }
}

#[test]
fn health_ok() {
    assert_eq!(run(vec!["health".into()]), ExitCode::SUCCESS);
}

#[test]
fn help_aliases() {
    for cmd in ["help", "--help", "-h"] {
        assert_eq!(run(vec![cmd.into()]), ExitCode::SUCCESS);
    }
}

#[test]
fn setup_help_exits_zero() {
    assert_eq!(
        run(vec!["setup".into(), "--help".into()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn setup_usage_errors_exit_2() {
    assert_eq!(
        run(vec!["setup".into(), "--bogus".into()]),
        ExitCode::from(2)
    );
}

#[test]
fn unknown_command_exits_2() {
    assert_eq!(run(vec!["nope".into()]), ExitCode::from(2));
}

#[test]
fn parse_flags_pairs_and_bare_flags() {
    let flags = flags_of(&["--db", "x", "--sessions"]);
    assert_eq!(flags.get("db").map(String::as_str), Some("x"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
    assert!(!flags.contains_key("missing"));
}

#[test]
fn parse_flags_bare_flag_before_another_flag() {
    // `--bind` has no value; `--db` must not be consumed as that value.
    let flags = flags_of(&["--bind", "--db", "sqlite::memory:", "--sessions"]);
    assert_eq!(flags.get("bind").map(String::as_str), Some("true"));
    assert_eq!(flags.get("db").map(String::as_str), Some("sqlite::memory:"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
}

#[test]
fn parse_flags_equals_syntax() {
    // `--flag=value` carries its own value; the value may itself contain `=`.
    let flags = flags_of(&[
        "--bind=127.0.0.1:34305",
        "--db=sqlite::memory:",
        "--sessions",
    ]);
    assert_eq!(
        flags.get("bind").map(String::as_str),
        Some("127.0.0.1:34305")
    );
    assert_eq!(flags.get("db").map(String::as_str), Some("sqlite::memory:"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
}

#[test]
fn parse_flags_equals_value_beats_following_flag() {
    // `--bind=X --db Y`: the token after `--bind=X` must not be swallowed.
    let flags = flags_of(&["--bind=X", "--db", "y"]);
    assert_eq!(flags.get("bind").map(String::as_str), Some("X"));
    assert_eq!(flags.get("db").map(String::as_str), Some("y"));
}

#[test]
fn database_help_block_masks_credentials() {
    let block = crate::cli_top_level::database_help_block(&|key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://user:s3cret@127.0.0.1:5432/am".into()),
        _ => None,
    });
    assert!(block.contains("postgres://***@127.0.0.1:5432/am"));
    assert!(!block.contains("s3cret"));
    assert!(block.contains("precedence:"));
}

/// The precedence line must name the variable the resolver reads today and must
/// not still offer the old spelling. Shape, not the sentence, because the
/// sentence will be worded again.
#[test]
fn the_precedence_line_names_only_the_prefixed_database_variable() {
    let block = crate::cli_top_level::database_help_block(&|_: &str| None);
    let line = block
        .lines()
        .find(|l| l.trim_start().starts_with("precedence:"))
        .expect("the block states a precedence order");
    let without_prefixed = line.replace("CLIO_DATABASE_URL", "");
    assert!(line.contains("CLIO_DATABASE_URL"), "{line}");
    assert!(!without_prefixed.contains("DATABASE_URL"), "{line}");
}

#[test]
fn globals_before_the_verb_are_routed() {
    // `--db` before the verb must still reach the read command.
    assert_eq!(
        run(vec![
            "--db".into(),
            "sqlite::memory:".into(),
            "recall".into(),
            "q".into()
        ]),
        ExitCode::SUCCESS
    );
}

#[test]
fn unknown_command_typo_exits_two() {
    assert_eq!(run(vec!["recal".into()]), ExitCode::from(2));
}

#[test]
fn help_json_exits_zero() {
    assert_eq!(run(vec!["help".into(), "--json".into()]), ExitCode::SUCCESS);
}

#[test]
fn read_command_unknown_flag_exits_two() {
    assert_eq!(
        run(vec![
            "recall".into(),
            "q".into(),
            "--limt".into(),
            "2".into(),
            "--db".into(),
            "sqlite::memory:".into()
        ]),
        ExitCode::from(2)
    );
}

#[test]
fn read_command_opens_and_reads_in_memory() {
    assert_eq!(
        run(vec![
            "stats".into(),
            "--db".into(),
            "sqlite::memory:".into(),
            "--output".into(),
            "json".into()
        ]),
        ExitCode::SUCCESS
    );
}
