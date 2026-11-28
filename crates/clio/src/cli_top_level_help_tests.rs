// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the top level and the reserved surfaces read a help
//! request out of raw arguments: which spellings mean help, and what a value
//! flag beside one does.
//!
//! ## Owns
//! - Coverage of `wants_help` and `cli_help_request::surface_help_requested`
//!   over the globals and a surface-local value flag
//!
//! - Coverage of the shared arity-aware scan behind them
//!
//! ## Does not own
//! - The rest of the top level's argument reading (`cli_top_level_tests`)
//! - The help screens a surface prints (`cli_help_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

/// `help` in any spelling asks for help, and nothing else does.
///
/// The three spellings are the ones a reader types, and a fourth one silently
/// treated as a flag is how `clio retention --help` once ran the command.
#[test]
fn help_is_recognised_in_every_spelling_and_nothing_else_is() {
    for spelling in ["help", "--help", "-h"] {
        assert!(
            wants_help(&[spelling.to_owned()]),
            "`{spelling}` must ask for help"
        );
    }
    for argv in [
        vec![],
        vec!["recall".into(), "q".into()],
        vec!["--helpful".into()],
    ] {
        assert!(!wants_help(&argv), "{argv:?} must not ask for help");
    }
}

/// A value flag's value is not a help request, and a value flag left without one
/// is the missing value rather than a help screen.
///
/// The parser refuses `--db -h` before any help path is reached, so the scan has
/// to agree: the token after a value flag is a flag, so there is no value for it
/// and no help request either. `--db help` stays a database called `help`, and an
/// explicit `--help` after a consumed value still asks.
#[test]
fn a_global_value_flags_value_is_not_a_help_request() {
    // Every declared global value flag, and the short `-o`, drops its value.
    let mut flags: Vec<String> = crate::cli_command_registry::global_value_names()
        .map(str::to_owned)
        .collect();
    flags.push("-o".to_owned());
    for flag in flags {
        assert!(
            !wants_help(&[flag.clone(), "help".to_owned()]),
            "`{flag} help` must not ask for help"
        );
        assert!(
            !wants_help(&[flag.clone(), "-h".to_owned()]),
            "`{flag} -h` is a missing value, so it asks for no help"
        );
        assert!(
            !wants_help(&[flag.clone(), "--help".to_owned()]),
            "`{flag} --help` is a missing value too"
        );
        assert!(
            wants_help(&[flag.clone(), "x".to_owned(), "--help".to_owned()]),
            "`{flag} x --help` has a value, so the help spelling still asks"
        );
    }
    // A word that is not a value still asks, and `--helpful` never does.
    assert!(wants_help(&["recall".to_owned(), "help".to_owned()]));
    assert!(!wants_help(&["--helpful".to_owned()]));
}

/// A surface-local value flag is read the same way a global is, so the two
/// surfaces answer one argv the same way: `--dir x -h` asks for help, while
/// `--dir -h` is the missing value the parser reports.
#[test]
fn a_surface_value_flag_is_read_the_way_a_global_is() {
    let ask = |argv: &[&str]| {
        let argv: Vec<String> = argv.iter().map(|token| (*token).to_owned()).collect();
        crate::cli_help_request::wants_help(&argv)
    };
    assert!(
        ask(&["--dir", "/tmp", "--help"]),
        "a consumed value then help"
    );
    assert!(ask(&["-h"]), "a lone help spelling");
    assert!(ask(&["--dir=/tmp", "-h"]), "the inline value is one token");
    assert!(!ask(&["--dir", "-h"]), "no value, so no help request");
    assert!(
        !ask(&["--dir", "--help"]),
        "an explicit --help is still no value"
    );
    assert!(!ask(&["--dir", "help"]), "`help` is the value of --dir");
    assert!(!ask(&["--", "-h"]), "a token after the delimiter is a word");
}

/// A lone `-` is a value, so the help spelling after it still asks, and a
/// surface flag whose value is that dash is not mistaken for a missing one.
#[test]
fn a_lone_dash_is_a_value_before_a_help_spelling() {
    assert!(wants_help(&[
        "--db".to_owned(),
        "-".to_owned(),
        "-h".to_owned()
    ]));
    let argv: Vec<String> = ["--file", "-", "--help"]
        .iter()
        .map(|t| (*t).to_owned())
        .collect();
    assert!(crate::cli_help_request::wants_help(&argv));
}

/// A valueless flag claims no token, so a help spelling after one is honoured.
///
/// The scan used to read any accepted flag name as a value flag, so
/// `mcp http --insecure-no-auth --help` swallowed the spelling and started a
/// server. Arity is what decides now: a toggle such as `--insecure-no-auth`,
/// `--shared-bank`, or a value-optional `--bind` leaves the next token alone.
#[test]
fn a_help_spelling_after_a_valueless_flag_is_honoured() {
    for argv in [
        vec!["--insecure-no-auth", "--help"],
        vec!["--insecure-no-auth", "-h"],
        vec!["--shared-bank", "-h"],
        vec!["--bind", "-h"],
        vec!["--insecure-bind-any", "--insecure-no-auth", "-h"],
        vec!["--skill-only", "-h"],
    ] {
        let argv: Vec<String> = argv.iter().map(|token| (*token).to_owned()).collect();
        assert!(
            crate::cli_help_request::wants_help(&argv),
            "{argv:?} names a help spelling after a valueless flag"
        );
    }
}
