// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the help screens and exit codes of the
//! `clio retention` command surface.
//!
//! ## Owns
//! - the help output of the `retention` surface and of each subcommand
//! - the exit code each `retention` outcome maps to
//! - the flag-arity rule that decides whether a help spelling asks for help
//!
//! ## Does not own
//! - the store round-trip and profile decoding (see `retention_cli_tests`)
//! - the shared flag parser (see `cli_top_level`)
//! - the help request scan (see `cli_help_request`)
//!
//! ## Boundary
//!
//! Changes to a retention help screen or exit code should normally require
//! changes here. Changes to the shared parser or the help scan should not.
//!
//! Keep this module focused on the retention command's help and exit codes.

use super::*;
use std::process::ExitCode;

#[test]
fn exit_codes_distinguish_usage_from_failure() {
    assert_eq!(
        retention(vec!["bogus".to_owned()]),
        ExitCode::from(2),
        "unknown subcommand is a usage error"
    );
    assert_eq!(
        retention(vec!["set".to_owned()]),
        ExitCode::from(1),
        "missing profile is a runtime failure"
    );
    assert_eq!(retention(vec!["schema".to_owned()]), ExitCode::SUCCESS);
    // `retention` with no subcommand at all is a usage error too.
    assert_eq!(retention(Vec::<String>::new()), ExitCode::from(2));
}

/// Each subcommand's help is the registry's usage line plus what that subcommand
/// does, so `clio retention get --help` cannot run the command instead.
///
/// The claim under test is the one the reported defect broke: `--help` was never
/// read here, so `retention get --help` reported the retention store missing.
#[test]
fn each_subcommands_help_carries_its_own_usage_and_prose() {
    for (sub, expected) in [
        ("get", "Shows the effective per-bank profile"),
        ("set", "Stores a validated profile"),
        ("schema", "Prints the retention profile JSON Schema"),
    ] {
        let text = super::retention_help(sub);
        assert!(
            text.contains(&format!("clio retention {sub}")),
            "`clio retention {sub} --help` must print its usage line: {text}"
        );
        assert!(
            text.contains(expected),
            "`clio retention {sub} --help` must say what it does: {text}"
        );
        // A help screen that never names a flag reads as a command that takes none.
        if sub != "schema" {
            assert!(
                text.contains("--"),
                "`clio retention {sub} --help` names no flag: {text}"
            );
        }
    }
}

/// The refusal carries the message and the hint on separate lines and exits 2.
///
/// An error that exits 0 is the defect this replaced: an unrecognised
/// subcommand used to report success.
#[test]
fn a_refusal_names_the_hint_and_exits_two() {
    assert_eq!(
        super::eprintln_error(
            "unknown retention command `bogus`",
            "try `clio retention get`"
        ),
        ExitCode::from(2)
    );
}

/// A help spelling where a subcommand belongs prints the subcommand list and
/// exits 0, so `clio retention --help` and `clio help retention` answer the
/// question instead of refusing `--help` as an unknown command.
#[test]
fn a_top_level_help_spelling_prints_the_subcommand_list() {
    for spelling in ["--help", "help", "-h"] {
        assert_eq!(
            retention(vec![spelling.to_owned()]),
            ExitCode::SUCCESS,
            "clio retention {spelling} must print help"
        );
    }
}

#[test]
fn an_undeclared_retention_flag_is_a_usage_error() {
    assert_eq!(
        retention(vec!["get".to_owned(), "--bogus".to_owned()]),
        ExitCode::from(2),
        "an unknown flag fails closed instead of running with defaults"
    );
}

/// A bare `help` token is one of the spellings the surface answers, and the help
/// scan runs before the parser, so a help request is never turned into a usage
/// error. `retention get help` printed help on master and must keep doing so.
#[test]
fn a_bare_help_token_prints_the_subcommand_help() {
    for sub in ["get", "set", "schema"] {
        assert_eq!(
            retention(vec![sub.to_owned(), "help".to_owned()]),
            ExitCode::SUCCESS,
            "{sub}"
        );
    }
}

/// `--profiles` needs a value, so a help spelling after it is not a help
/// request: the scan declines and the parser reports the missing value.
///
/// The scan here used to match the token regardless of what came before it, so
/// `retention get --profiles -h` printed a usage line while `version --check -h`
/// reported the missing value for the same shape.
#[test]
fn a_help_spelling_after_profiles_is_the_missing_value() {
    for spelling in ["-h", "--help"] {
        let collected: Vec<String> = vec!["--profiles".to_owned(), spelling.to_owned()];
        assert!(
            !crate::cli_help_request::wants_help(&collected),
            "`--profiles {spelling}` is no help request"
        );
        let error = crate::cli_top_level::parse_flags(
            collected.iter().cloned(),
            &super::retention_known_flags("get"),
        )
        .expect_err("`--profiles` claims the token after it, so there is no value");
        assert_eq!(error.code(), "usage", "{spelling}");
        assert_eq!(error.message(), "flag `--profiles` requires a value");
    }
    // With `--bank` given its value, the spelling behind the pair asks for help.
    let bank_only: Vec<String> = vec!["--bank".to_owned(), "b".to_owned(), "-h".to_owned()];
    assert!(crate::cli_help_request::wants_help(&bank_only));
}
