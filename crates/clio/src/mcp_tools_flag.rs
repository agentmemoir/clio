// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `--tools` flag of the two `clio mcp`
//! transports: the value it accepts, the refusal a bad value raises, its help
//! block, and the server state the resolved selection opens.
//!
//! ## Owns
//! - Decoding `--tools` into the tool groups the transports expose
//! - The usage refusal (exit 2) for an unknown preset, a malformed value, or an
//!   unknown group inside `custom:`
//! - The `--tools` help block both transport help screens print
//! - Opening the server state under a `--tools` selection and a `--discovery`
//!   switch, ahead of the config settings
//!
//! ## Does not own
//! - The group, preset, and closure data or their derivations (see `clio-types`)
//! - The precedence between a selection, `mcp.tool_profile`, and the default
//!   preset (see `clio_config::config::Runtime::mcp_tool_groups`)
//! - Decoding `--discovery` (see `mcp_discovery_flag`)
//! - Flag arity and the accepted-name lists (see the surface records)
//!
//! ## Boundary
//!
//! Changes to what `--tools` accepts or prints should normally require changes
//! here. Changes to which tools a group holds should not.
//!
//! Keep this module focused on the `--tools` flag and the shared opener.

use crate::cli_error::CliError;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_types::{ToolGroup, ToolPreset, custom};
use std::collections::HashMap;
use std::sync::Arc;

/// The prefix that starts a caller-supplied group list instead of a preset name.
const CUSTOM_PREFIX: &str = "custom:";

/// The `--tools` block both transport help screens print.
pub(crate) const HELP_BLOCK: &str = "\
--tools PRESET
             Tool groups the server advertises: `minimal`, `coding`, `ops`,
             `all`, or custom:GROUP[,GROUP] for the minimal set plus the listed
             groups and what they require. With no --tools the config setting
             `mcp.tool_profile` applies; when it is unset the `minimal` preset
             applies.";

/// The groups `flags` selects, or `None` when the transport was given no
/// `--tools` and the configured setting decides.
///
/// # Errors
/// The value names no preset this build knows, is not formed, or names no tool
/// group inside `custom:`.
pub(crate) fn selection(
    flags: &HashMap<String, String>,
) -> Result<Option<Vec<ToolGroup>>, CliError> {
    flags.get("tools").map(|value| groups(value)).transpose()
}

/// The groups one `--tools` value names: a preset's resolved set, or `custom:`
/// and a comma-separated group list resolved from the minimal set.
fn groups(value: &str) -> Result<Vec<ToolGroup>, CliError> {
    let Some(selected) = value.strip_prefix(CUSTOM_PREFIX) else {
        return ToolPreset::from_str(value)
            .map(ToolPreset::resolve)
            .ok_or_else(|| {
                CliError::usage_hint(
                    format!("--tools `{value}` is not a preset this build knows"),
                    format!(
                        "use one of {}, or custom:<group>[,<group>]",
                        ToolPreset::ALL.map(ToolPreset::as_str).join("|")
                    ),
                )
            });
    };
    let names: Vec<&str> = selected.split(',').map(str::trim).collect();
    if names.iter().any(|name| name.is_empty()) {
        return Err(CliError::usage_hint(
            format!("--tools `{value}` names no tool group"),
            groups_hint(),
        ));
    }
    custom(&names)
        .map_err(|error| CliError::usage_hint(format!("--tools `{value}`: {error}"), groups_hint()))
}

/// The hint a malformed or unknown group selection carries.
fn groups_hint() -> String {
    format!(
        "name the groups as comma-separated wire words: {}",
        ToolGroup::ALL.map(ToolGroup::as_str).join(",")
    )
}

/// Open the server state under `groups` and `discovery`, when the transport
/// selected either.
///
/// Without a selection this is [`McpState::open`] with the same env-aware
/// runtime, so the configured `mcp.tool_profile` keeps deciding. Each selection
/// is an absolute override on its own axis, so an installed value wins even
/// when it turns the configured setting off.
///
/// # Errors
/// As [`McpState::open`].
pub(crate) fn open_state(
    opts: &McpOpenOptions,
    groups: Option<&[ToolGroup]>,
    discovery: Option<bool>,
) -> Result<Arc<McpState>, clio_types::AmError> {
    open_state_with_runtime(opts, clio_config::Runtime::new(), groups, discovery)
}

/// [`open_state`] over a caller-supplied runtime, so a test can name the whole
/// configuration layer instead of inheriting the host's environment.
///
/// # Errors
/// As [`McpState::open`].
pub(crate) fn open_state_with_runtime(
    opts: &McpOpenOptions,
    mut runtime: clio_config::Runtime,
    groups: Option<&[ToolGroup]>,
    discovery: Option<bool>,
) -> Result<Arc<McpState>, clio_types::AmError> {
    if let Some(groups) = groups {
        runtime.set_mcp_tool_groups(groups.to_vec());
    }
    if let Some(discovery) = discovery {
        runtime.set_mcp_discovery(Some(discovery));
    }
    McpState::open_with_runtime(opts, runtime)
}

#[cfg(test)]
#[path = "mcp_tools_flag_tests.rs"]
mod tests;
