// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio mcp` entry point's own dispatch branches: the help
//! spellings, and the fail-closed flag check on a subcommand.
//!
//! ## Owns
//! - Dispatch and exit-code tests for the `mcp` entry point's own branches
//!
//! ## Does not own
//! - The subcommand handlers and flag decoding (`mcp_cli_tests`)
//! - The credential-store load (`mcp_cli_credentials_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a `clio mcp` dispatch branch or its exit code changes;
//! they never change it.
//!
//! Keep this module focused on the `clio mcp` dispatch surface.

use super::tests::args_of;
use super::*;

/// Every help spelling the `mcp` entry point accepts prints the subcommand list
/// and succeeds, which is what `clio mcp --help` used to fail to do.
#[test]
fn a_help_spelling_lists_the_subcommands() {
    for spelling in ["help", "--help", "-h"] {
        assert_eq!(mcp(args_of(&[spelling])), ExitCode::SUCCESS, "{spelling}");
    }
}

/// An undeclared flag on a subcommand fails closed, rather than printing the
/// whole schema pack as if the flag had been understood.
#[test]
fn schema_export_rejects_an_undeclared_flag() {
    assert_eq!(
        mcp(args_of(&["schema-export", "--bogus"])),
        ExitCode::from(2)
    );
}

/// An undeclared flag on the `stdio` transport is a usage error (exit 2), checked
/// before the store is opened; it used to surface as an exit-1 open failure.
#[test]
fn stdio_rejects_an_undeclared_flag() {
    assert_eq!(mcp(args_of(&["stdio", "--bogus"])), ExitCode::from(2));
}

/// A flag only the HTTP transport takes is refused on `stdio` rather than
/// silently ignored, so a wrong flag cannot look like a working invocation.
#[test]
fn stdio_rejects_an_http_only_flag() {
    assert_eq!(
        mcp(args_of(&["stdio", "--bind", "127.0.0.1:1"])),
        ExitCode::from(2)
    );
}

/// The removed `--auth-token` flag is now an unknown flag on `http`: a usage
/// error checked before the store is opened, not a silent credential.
#[test]
fn http_rejects_the_removed_auth_token_flag() {
    assert_eq!(
        mcp(args_of(&["http", "--auth-token", "x"])),
        ExitCode::from(2)
    );
}
