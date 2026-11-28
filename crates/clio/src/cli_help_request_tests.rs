// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared help-request scan and its value-flag skip.
//!
//! ## Owns
//! - Assertions over `help_requested`, `is_value_flag`, and `wants_help`
//!
//! ## Does not own
//! - The help screens a surface prints (see `cli_top_level_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

/// Treats `--db` as the only value flag, so the skip can be read directly.
fn db_is_a_value_flag(arg: &str) -> bool {
    arg == "--db"
}

/// `help` in every spelling asks for help, and nothing else does.
#[test]
fn help_is_recognised_in_every_spelling_and_nothing_else_is() {
    for spelling in ["help", "--help", "-h"] {
        assert!(
            help_requested(&[spelling.to_owned()], &db_is_a_value_flag),
            "`{spelling}` must ask for help"
        );
    }
    for argv in [vec![], vec!["--helped".to_owned()]] {
        assert!(
            !help_requested(&argv, &db_is_a_value_flag),
            "{argv:?} must not ask for help"
        );
    }
}

/// A value flag's value is not a help request: the flag and the token after it
/// are skipped, so `--db help` names a database called `help`. A single-dash
/// token is a flag, never a value, so `--db -h` leaves `--db` with no value at
/// all: the missing-value refusal the parser reports, not a help request.
#[test]
fn a_value_flags_value_is_not_a_help_request() {
    assert!(
        !help_requested(&["--db".to_owned(), "help".to_owned()], &db_is_a_value_flag),
        "`--db help` must not ask for help"
    );
    assert!(
        !help_requested(&["--db".to_owned(), "-h".to_owned()], &db_is_a_value_flag),
        "`-h` is a flag, so `--db` has no value and no help is asked for"
    );
    // A token after the flag that is itself a flag is not its value either, so
    // an explicit `--help` there leaves the value flag without one.
    assert!(!help_requested(
        &["--db".to_owned(), "--help".to_owned()],
        &db_is_a_value_flag
    ));
    assert!(help_requested(
        &["--db".to_owned(), "x".to_owned(), "--help".to_owned()],
        &db_is_a_value_flag
    ));
}

/// Tokens at or after a `--` delimiter are positional, so a help spelling
/// behind it is not a request.
#[test]
fn a_help_spelling_after_the_delimiter_is_not_a_request() {
    for argv in [
        vec!["health", "--", "--help"],
        vec!["--", "--help"],
        vec!["--", "--json"],
        vec!["health", "--", "-h"],
    ] {
        let argv = argv
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        assert!(
            !help_requested(&argv, &db_is_a_value_flag),
            "{argv:?} must not ask for help"
        );
    }
    assert!(help_requested(
        &["health".to_owned(), "--help".to_owned()],
        &db_is_a_value_flag
    ));
}

/// `is_value_flag` reads a flag's arity, so a toggle and a value-optional flag
/// both leave the token after them alone while a value flag claims it.
///
/// This is the predicate the scan shares with the parsers, so `--db` claiming the
/// next token and `--insecure-no-auth` not claiming it cannot drift apart.
#[test]
fn the_value_flag_predicate_reads_arity() {
    for flag in ["--db", "--bank", "--backend", "--actor", "--output", "-o"] {
        assert!(is_value_flag(flag), "`{flag}` takes a value");
    }
    for flag in [
        "--help",
        "-h",
        "help",
        "--insecure-no-auth",
        "--shared-bank",
        "--bind",
        "--insecure-bind-any",
        "--force",
        "--skill-only",
        "--yes",
        "--no-input",
    ] {
        assert!(!is_value_flag(flag), "`{flag}` claims no value");
    }
    // A surface-local value flag is a value flag too, and a name no record
    // declares is not one.
    assert!(is_value_flag("--dir"));
    assert!(is_value_flag("--harness"));
    assert!(is_value_flag("--profiles"));
    assert!(!is_value_flag("--not-a-declared-flag"));
}

/// A help spelling after a valueless flag still asks, which is the rule that
/// keeps a boolean surface flag from swallowing the request: `--yes -h` prints
/// help while `--db -h` reports the missing value.
#[test]
fn a_help_spelling_after_a_valueless_flag_is_a_request() {
    for argv in [
        vec!["--yes", "-h"],
        vec!["--insecure-no-auth", "--help"],
        vec!["--shared-bank", "-h"],
        vec!["--bind", "-h"],
    ] {
        let argv = argv
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        assert!(wants_help(&argv), "{argv:?} must ask for help");
    }
    assert!(!wants_help(&["--db".to_owned(), "-h".to_owned()]));
}
