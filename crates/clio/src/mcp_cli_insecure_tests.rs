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

/// The non-loopback opt-out is a second toggle of its own: off by default, and
/// off unless its own value is `true`, so naming one flag never sets the other.
#[test]
fn the_bind_any_toggle_is_off_by_default_and_follows_its_value() {
    assert!(!config_with(&[]).insecure_bind_any, "off by default");
    assert!(
        !config_with(&["--insecure-bind-any=false"]).insecure_bind_any,
        "a false value stays off"
    );
    assert!(
        config_with(&["--insecure-bind-any"]).insecure_bind_any,
        "the bare flag turns it on"
    );
    assert!(
        config_with(&["--insecure-bind-any=true"]).insecure_bind_any,
        "a true value turns it on"
    );
    let one_flag_only = config_with(&["--insecure-no-auth"]);
    assert!(
        !one_flag_only.insecure_bind_any,
        "--insecure-no-auth must not imply the bind opt-out"
    );
}

/// The help block that documents `flag`, selected by its own leading line so the
/// usage line (which names every flag) cannot stand in for the prose.
fn help_block<'a>(help: &'a str, flag: &str) -> &'a str {
    help.split("\n\n")
        .find(|block| block.starts_with(flag))
        .unwrap_or_else(|| panic!("no {flag} block in the help: {help}"))
}

/// The help names the second opt-out where an operator looks for it: the
/// `--bind` paragraph says a non-loopback bind needs it, the `--token-file`
/// paragraph says the store is required except under the opt-out and that the
/// opt-out needs it outside loopback, and the `--insecure-no-auth` paragraph
/// says it does not reach past loopback.
#[test]
fn the_help_says_a_non_loopback_bind_needs_the_bind_any_flag() {
    let help = mcp_http_help();
    assert!(help.contains("--insecure-bind-any"), "{help}");
    let bind_para = help_block(&help, "--bind ADDR");
    assert!(
        bind_para.contains("--insecure-bind-any"),
        "the --bind text must name the flag: {bind_para}"
    );
    assert!(
        bind_para.contains("credential"),
        "the --bind text must still name the credential rule: {bind_para}"
    );
    let token_para = help_block(&help, "--token-file");
    assert!(
        token_para.contains("--insecure-no-auth"),
        "the --token-file text must name the opt-out: {token_para}"
    );
    assert!(
        token_para.contains("--insecure-bind-any"),
        "the --token-file text must name the non-loopback rule too: {token_para}"
    );
    let no_auth_para = help_block(&help, "--insecure-no-auth");
    assert!(
        no_auth_para.contains("does not permit a non-loopback bind"),
        "the --insecure-no-auth text must deny the wider bind: {no_auth_para}"
    );
    let bind_any_para = help_block(&help, "--insecure-bind-any");
    assert!(
        bind_any_para.contains("non-loopback address"),
        "the new flag's text must say what it unlocks: {bind_any_para}"
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
