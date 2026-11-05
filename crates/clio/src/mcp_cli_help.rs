// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `--help` screens of the two `clio mcp`
//! transports: their usage line, the flags only their prose explains, and the
//! tool-selection and database blocks they share.
//!
//! ## Owns
//! - `mcp_stdio_help` and `mcp_http_help`, the two screens as text
//! - The statements about the auto-bind scan, the credential rules, and the
//!   tool selection's default
//!
//! ## Does not own
//! - The flags themselves or which names a transport accepts (see the surface
//!   records and `mcp_cli`)
//! - The `--tools` value and its help block (see `mcp_tools_flag`)
//! - The database precedence block (see `cli_top_level::database_help_block`)
//!
//! ## Boundary
//!
//! Changes to what a transport's `--help` says should normally require changes
//! here. Changes to what a transport accepts should not.
//!
//! Keep this module focused on the two transports' help screens.

use crate::cli_help_usage::usage_line;
use crate::cli_top_level::database_help_block;
use crate::mcp_tools_flag::HELP_BLOCK as TOOLS_HELP;

/// `clio mcp stdio` usage text: the record's usage line, the database block, and
/// the tool selection.
pub(crate) fn mcp_stdio_help() -> String {
    format!(
        "{}\n\n{}\n\n{}",
        usage_line("mcp stdio").unwrap_or_else(|| "clio mcp stdio".to_owned()),
        database_help_block(&|key| std::env::var(key).ok()),
        TOOLS_HELP
    )
}

/// `clio mcp http` usage text, including the auto-bind rule.
pub(crate) fn mcp_http_help() -> String {
    let range = format!(
        "{}-{}",
        crate::port_scan::FIRST_PORT,
        crate::port_scan::LAST_PORT
    );
    format!(
        "{}\n\n--bind ADDR  Bind address. With no flag, or with a bare `--bind`, scan 127.0.0.1:{range}
             for the first free port. An explicit `--bind HOST:PORT` binds exactly
             that address; a non-loopback address also requires a credential, or
             both --insecure-no-auth and --insecure-bind-any.

--token-file PATH
             Credential store whose records name an actor and the banks it may reach
             (see `clio auth token add`). Loaded once at start; a path that is
             absent, unreadable, malformed, or holds no record refuses the start.
             Required on every bind unless --insecure-no-auth is given; outside
             loopback that opt-out also needs --insecure-bind-any.

--insecure-no-auth
             Permit a bind with no credential. Without it a credential is
             required everywhere, including loopback: name a --token-file store,
             or the start is refused. A --token-file store, when given, is still
             enforced. It does not permit a non-loopback bind: that needs
             --insecure-bind-any as well.

--insecure-bind-any
             Permit --insecure-no-auth to bind a non-loopback address. The start
             then warns that every host that can reach the port can use every
             tool. Without it a non-loopback bind with no credential is refused
             even with --insecure-no-auth.

{TOOLS_HELP}

{}",
        usage_line("mcp http").unwrap_or_else(|| "clio mcp http".to_owned()),
        database_help_block(&|key| std::env::var(key).ok())
    )
}
