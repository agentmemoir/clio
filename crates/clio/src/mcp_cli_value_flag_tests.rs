// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what a `clio mcp` subcommand reads a help request as: which
//! spellings mean help, and what a value flag beside one does.
//!
//! ## Owns
//! - Coverage of `wants_help` over a transport's own flags and the globals
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
//! Keep this module focused on the transports' help request reading.

use super::tests::args_of;
use super::wants_help;
use crate::cli_command_registry_surface::parse_flag_names;

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
