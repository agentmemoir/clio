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
//! - The help text's promise of the token store and the opt-out flag
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

/// The toggle is off by default and follows its parsed value: `=false` stays
/// off, a bare flag and `=true` turn it on.
#[test]
fn the_insecure_toggle_is_off_by_default_and_follows_its_value() {
    assert!(!config_with(&[]).insecure_no_auth, "off by default");
    assert!(
        !config_with(&["--insecure-no-auth=false"]).insecure_no_auth,
        "a false value stays off"
    );
    assert!(
        config_with(&["--insecure-no-auth"]).insecure_no_auth,
        "the bare flag turns it on"
    );
    assert!(
        config_with(&["--insecure-no-auth=true"]).insecure_no_auth,
        "a true value turns it on"
    );
}

/// The help names the opt-out and the required token store, and advertises
/// neither a minted token nor a removed source.
#[test]
fn the_help_names_the_required_token_store_and_the_opt_out_flag() {
    let help = mcp_http_help();
    assert!(help.contains("--insecure-no-auth"), "{help}");
    assert!(help.contains("--token-file"), "{help}");
    for removed in ["--auth-token", "CLIO_MCP_TOKEN", "one-time bearer token"] {
        assert!(
            !help.contains(removed),
            "the help still advertises {removed}: {help}"
        );
    }
}
