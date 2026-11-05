// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the two `clio mcp` transports wire their `--tools`
//! selection: a value no preset and no group list can honour stops the start
//! with a usage error, and both `--help` screens state the default that applies
//! when no flag is given.
//!
//! ## Owns
//! - The usage refusal (exit 2) reaching both transport entry points, naming the
//!   value it refused
//! - The `--tools` block of both help screens
//!
//! ## Does not own
//! - What the flag accepts and the groups it yields (`mcp_tools_flag_tests`)
//! - The transports themselves (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a transport consumes the selection should normally require
//! changes here. Changes to the accepted values should not.
//!
//! Keep this module focused on the `--tools` wiring.

use super::tests::args_of;
use super::*;

/// A `--tools` value no preset and no group list can honour is refused before
/// any transport opens a store, so no server starts on a value it cannot serve
/// and none advertises a set it guessed.
#[test]
fn a_bad_tools_value_stops_both_transports_with_a_usage_error() {
    for value in ["mnimal", "custom:ops,bogus", "custom:"] {
        for transport in ["stdio", "http"] {
            let code = mcp(args_of(&[transport, "--tools", value]));
            assert_eq!(code, ExitCode::from(2), "{transport} served `{value}`");
        }
    }
}

/// The refusal a bad value raises names the value that was written, so a caller
/// can see which part of it this build does not know, and yields no groups a
/// state could be opened with.
#[test]
fn a_bad_tools_value_is_refused_naming_it_and_yields_no_groups() {
    for (value, offender) in [
        ("mnimal", "mnimal"),
        ("custom:ops,bogus", "bogus"),
        ("custom:", "custom:"),
    ] {
        let error = crate::mcp_tools_flag::selection(&crate::cli_flags_test_support::flags_of(&[
            "--tools", value,
        ]))
        .expect_err("the value names no set this build can build");
        assert_eq!(error.exit_code(), ExitCode::from(2), "`{value}`");
        assert!(error.message().contains(offender), "{}", error.message());
    }
}

/// Both `--help` screens state what `--tools` takes and what it does not: the
/// preset names, the `custom:` form, and the config setting that decides when
/// no flag is given, with `minimal` as the fallback.
#[test]
fn both_transport_help_screens_state_the_tools_default() {
    for help in [mcp_stdio_help(), mcp_http_help()] {
        assert!(help.contains("--tools PRESET"), "{help}");
        for preset in ["`minimal`", "`coding`", "`ops`", "`all`"] {
            assert!(help.contains(preset), "`{preset}` is not offered: {help}");
        }
        assert!(help.contains("custom:GROUP[,GROUP]"), "{help}");
        assert!(help.contains("mcp.tool_profile"), "{help}");
        assert!(help.contains("`minimal` preset"), "{help}");
    }
}
