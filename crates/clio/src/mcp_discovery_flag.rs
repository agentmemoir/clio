// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `--discovery` flag of the two `clio mcp`
//! transports: the values it accepts, the refusal a bad value raises, and its
//! help block.
//!
//! ## Owns
//! - Decoding `--discovery` into the switch that adds the three discovery tools
//! - The usage refusal (exit 2) for a value other than `on` or `off`
//! - The `--discovery` help block both transport help screens print
//!
//! ## Does not own
//! - The precedence between the flag and `mcp.discovery` (see
//!   `clio_config::config::Runtime::mcp_discovery`)
//! - The discovery tool declarations or their listing entries (see `clio-mcp`)
//! - Opening the server state (see `mcp_tools_flag::open_state`)
//!
//! ## Boundary
//!
//! Changes to what `--discovery` accepts or prints should normally require
//! changes here. Changes to which tools discovery adds should not.
//!
//! Keep this module focused on the `--discovery` flag.

use crate::cli_error::CliError;
use std::collections::HashMap;

/// The `--discovery` block both transport help screens print.
pub(crate) const HELP_BLOCK: &str = "\
--discovery on|off
             Advertise the three discovery tools (`tool_search`, `tool_describe`,
             and `tool_invoke`). With no --discovery the config setting
             `mcp.discovery` applies; when it is unset they stay off.";

/// The switch `flags` selects, or `None` when the transport was given no
/// `--discovery` and the configured setting decides.
///
/// # Errors
/// The value is not `on` or `off`.
pub(crate) fn selection(flags: &HashMap<String, String>) -> Result<Option<bool>, CliError> {
    flags.get("discovery").map(|value| parse(value)).transpose()
}

/// The switch one `--discovery` value names: `on` or `off`, nothing else.
fn parse(value: &str) -> Result<bool, CliError> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(CliError::usage_hint(
            format!("--discovery `{value}` is not on or off"),
            "pass --discovery on to advertise the discovery tools, or --discovery off to hide them",
        )),
    }
}

#[cfg(test)]
#[path = "mcp_discovery_flag_tests.rs"]
mod tests;
