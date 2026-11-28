// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what a `clio mcp` subcommand reads a help request and a
//! value flag as: which spellings mean help, and what a value flag whose value is
//! missing reports.
//!
//! ## Owns
//! - Coverage of `wants_help` over a transport's own flags and the globals
//! - Coverage of the missing-value refusal a transport's value flag raises
//!
//! ## Does not own
//! - The help screens themselves (`mcp_cli_help_tests`)
//! - The transports' dispatch and exit codes (`mcp_cli_dispatch_tests`)
//! - The credential store `--token-file` loads (`mcp_cli_credentials_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the transports' help and value-flag reading.

use super::tests::{args_of, flags_of};
use super::wants_help;
use crate::cli_command_registry_surface::parse_flag_names;
use crate::cli_top_level::parse_flags;

/// The accepted names of one transport, read the way its dispatcher reads them.
fn http_flags() -> Vec<&'static str> {
    parse_flag_names("mcp http")
}

#[test]
fn wants_help_spots_both_spellings_anywhere_in_the_arguments() {
    let known = http_flags();
    assert!(wants_help(&args_of(&["--bank", "b", "--help"]), &known));
    assert!(wants_help(&args_of(&["-h"]), &known));
    assert!(!wants_help(&args_of(&["--bank", "b"]), &known));
    assert!(!wants_help(&args_of(&["--helped"]), &known));
}

#[test]
fn wants_help_stops_at_the_delimiter() {
    // A help flag after `--` is an operand, so it prints no help.
    let known = http_flags();
    assert!(!wants_help(
        &args_of(&["--bank", "b", "--", "--help"]),
        &known
    ));
    assert!(!wants_help(&args_of(&["--", "-h"]), &known));
    assert!(wants_help(
        &args_of(&["--help", "--", "--bank", "b"]),
        &known
    ));
}

/// A transport's own value flag is read the way a global is: its value is not a
/// help spelling, and with no value behind it the argv is the missing-value
/// refusal, never a help screen.
#[test]
fn a_transport_value_flag_left_without_a_value_is_not_a_help_request() {
    let known = http_flags();
    assert!(wants_help(
        &args_of(&["--token-file", "store", "-h"]),
        &known
    ));
    assert!(!wants_help(&args_of(&["--token-file", "-h"]), &known));
    assert!(!wants_help(&args_of(&["--token-file", "help"]), &known));
    // `--bind` is value-optional, so a token after it is its value and the help
    // spelling behind that one still asks.
    assert!(wants_help(
        &args_of(&["--bind", "127.0.0.1:4000", "-h"]),
        &known
    ));
}

/// A transport's own value flag left without one is refused before any
/// credential store is resolved, so no run opens a store named after the token
/// that followed the flag.
#[test]
fn a_transport_value_flag_followed_by_a_flag_is_a_usage_error() {
    let names = http_flags();
    for (argv, flag) in [
        (vec!["--token-file", "-x"], "--token-file"),
        (vec!["--token-file"], "--token-file"),
        (vec!["--token-file", "--db", "x"], "--token-file"),
        (vec!["--out", "-x"], "--out"),
    ] {
        let error = parse_flags(argv.iter().map(|token| (*token).to_owned()), &names)
            .expect_err("a value flag with a flag after it is a missing value");
        assert_eq!(error.code(), "usage", "{argv:?}");
        assert_eq!(
            error.message(),
            format!("flag `{flag}` requires a value"),
            "{argv:?}"
        );
    }
    // A lone `-` is still a value, and `--bind` keeps its documented bare form.
    let dash =
        parse_flags(args_of(&["--token-file", "-"]), &names).expect("a lone dash is a value");
    assert_eq!(dash.get("token-file").map(String::as_str), Some("-"));
    assert_eq!(
        flags_of(&["--bind"]).get("bind").map(String::as_str),
        Some("true")
    );
}

/// A store reflecting whether anything created a file named after a swallowed
/// token cannot be built here, so the refusal is asserted at the seam the
/// dispatcher uses: the flag map never holds the token as a value.
#[test]
fn the_swallowed_token_never_reaches_the_flag_map() {
    let names = http_flags();
    let error = parse_flags(args_of(&["--token-file", "-x"]), &names)
        .expect_err("the token is not a value");
    assert_eq!(error.message(), "flag `--token-file` requires a value");
    assert_eq!(error.hint(), Some("try `--token-file VALUE`"));
}
