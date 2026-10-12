// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared help-request scan and its value-flag skip.
//!
//! ## Owns
//! - Assertions over `help_requested`
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
/// are skipped, so `--db help` names a database called `help`.
#[test]
fn a_value_flags_value_is_not_a_help_request() {
    for value in ["help", "-h"] {
        assert!(
            !help_requested(&["--db".to_owned(), value.to_owned()], &db_is_a_value_flag),
            "`--db {value}` must not ask for help"
        );
    }
    // A token after the flag that starts with `--` is not its value, so an
    // explicit `--help` after a value flag still asks.
    assert!(help_requested(
        &["--db".to_owned(), "--help".to_owned()],
        &db_is_a_value_flag
    ));
    assert!(help_requested(
        &["--db".to_owned(), "x".to_owned(), "--help".to_owned()],
        &db_is_a_value_flag
    ));
}
