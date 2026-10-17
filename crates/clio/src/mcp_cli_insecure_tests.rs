// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `--insecure-no-auth` opt-out of `clio mcp http`: that
//! it is off by default, turns on only when given, and is named by the help.
//!
//! ## Owns
//! - The `http_config` decoding of the toggle
//! - The help text's promise of the minted token and the opt-out flag
//!
//! ## Does not own
//! - The `--sessions` value rule (`mcp_cli_sessions_tests`)
//! - The credential-store load (`mcp_cli_credentials`)
//! - The transport's own start guard (`clio-mcp`'s `http`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::config_with;
use super::*;

/// The toggle is off unless it is given, and a valued spelling does not turn it
/// on: only a bare `--insecure-no-auth` is the explicit opt-out.
#[test]
fn the_insecure_toggle_defaults_off_and_only_a_bare_flag_turns_it_on() {
    assert!(!config_with(&[]).insecure_no_auth, "off by default");
    assert!(
        !config_with(&["--insecure-no-auth=false"]).insecure_no_auth,
        "a valued spelling stays off"
    );
    assert!(
        config_with(&["--insecure-no-auth"]).insecure_no_auth,
        "the bare flag turns it on"
    );
}

/// The help names the opt-out and the minted token, and still advertises
/// neither removed source.
#[test]
fn the_help_names_the_minted_token_and_the_opt_out_flag() {
    let help = mcp_http_help();
    assert!(help.contains("--insecure-no-auth"), "{help}");
    assert!(help.contains("one-time bearer token"), "{help}");
    for removed in ["--auth-token", "CLIO_MCP_TOKEN"] {
        assert!(
            !help.contains(removed),
            "the removed source {removed} is advertised: {help}"
        );
    }
}
