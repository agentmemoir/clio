// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the CLI help catalog and command suggestion:
//! the machine-readable `help --json` catalog derived from the MCP schema pack,
//! the verb → tool binding table, and the `did you mean` lookups.
//!
//! ## Owns
//! - The CLI command → MCP tool binding lookup `command_bindings` (including
//!   two-token groups), answered in the global order `cli_command_bindings`
//!   holds
//! - `help --json` catalog assembly from `clio_mcp::schema::schema_pack()`
//! - The commands the CLI accepts that bind no MCP tool, and so are listed in
//!   the catalog separately from the binding table
//! - Unknown-command/subcommand suggestion and per-command usage lookup
//! - The derived group and prefix lists (see `cli_help_groups`)
//!
//! ## Does not own
//! - The binding table's global order (see `cli_command_bindings`)
//! - The top-level human help layout (see the binary's `print_help`)
//! - Tool schemas or dispatch (see `clio-mcp`)
//! - Per-command usage strings (see `cli_help_usage`) or argument parsing (`cli_args`)
//!
//! ## Boundary
//!
//! Changes to the CLI catalog or command bindings should normally require changes here.
//! Changes to a tool's schema should not require changes here.
//!
//! Keep this module focused on the CLI help catalog and suggestion.

use crate::cli_args::suggest;
use serde_json::{Value, json};
use std::io::{self, Write};

#[cfg(test)]
pub(crate) use self::groups::all_known_commands;
/// The derived group and prefix lists (see the `groups` submodule). The
/// completeness list is test-only, because the completeness checks are.
pub(crate) use self::groups::{group_subcommands, known_commands, shared_groups, subcommands_of};

/// The CLI command → MCP tool binding, in whole-catalog order.
///
/// The order and the resolution live in `cli_command_bindings`: a key prints
/// its registry record's tool when it binds one, else the row the surface
/// still holds statically.
pub(crate) fn command_bindings() -> Vec<(&'static str, &'static str)> {
    crate::cli_command_bindings::command_bindings()
}

/// Whether the CLI accepts `name` as a command, as `group subcommand`, or as
/// `group subcommand verb`.
///
/// The three depths `clio help <topic>` may name. `backup create` is two tokens
/// and `auth token add` is three, so a topic check that only knew the top-level
/// verbs would refuse two commands the CLI runs happily.
pub(crate) fn is_known_command(name: &str) -> bool {
    let mut words = name.split(' ');
    let Some(verb) = words.next() else {
        return false;
    };
    let rest: Vec<&str> = words.collect();
    match rest.as_slice() {
        [] => known_commands().contains(&verb),
        [sub] => subcommands_of(verb).is_some_and(|subs| subs.contains(sub)),
        // `sync key export` and `sync key status` are three-token commands whose
        // two-word subcommand sits in the group table; `auth token add` names
        // the two verbs of `auth token`, a group whose subcommand is itself a
        // group, so its subs are looked up under the two-token prefix.
        [sub, verb2] => {
            subcommands_of(&format!("{verb} {sub}")).is_some_and(|subs| subs.contains(verb2))
                || subcommands_of(verb).is_some_and(|subcommands| {
                    subcommands
                        .iter()
                        .any(|name| name.split_once(' ') == Some((*sub, *verb2)))
                })
        }
        _ => false,
    }
}

/// The closest known top-level command to `input`, if one is close enough.
pub(crate) fn suggest_command(input: &str) -> Option<&'static str> {
    suggest(input, &known_commands())
}

/// Usage line for a group's `--help`, listing its subcommands and bound tools.
pub(crate) fn group_help(cmd: &str, subcommands: &[&str]) -> String {
    let mut lines = vec![
        format!("usage: clio {cmd} <subcommand> [flags]"),
        String::new(),
        format!("`{cmd}` subcommands:"),
    ];
    for sub in subcommands {
        let key = format!("{cmd} {sub}");
        match bound_tool(&key) {
            Some(tool) => lines.push(format!("  clio {key}   -> {tool}")),
            // A group subcommand that binds no MCP tool (e.g. `backup dir`)
            // says so instead of rendering a meaningless `-> ?`.
            None => lines.push(format!("  clio {key}   (no MCP tool)")),
        }
    }
    lines.join("\n")
}

/// The MCP tool bound to a canonical command, if any.
pub(crate) fn bound_tool_name(command: &str) -> Option<&'static str> {
    bound_tool(command)
}

/// The MCP tool bound to a canonical command, if any.
fn bound_tool(command: &str) -> Option<&'static str> {
    command_bindings()
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, tool)| *tool)
}

/// Every command this surface knows that binds no MCP tool, sorted.
///
/// The `commands` array is built from the binding table, so a command that
/// binds no tool is absent from the catalog even though the CLI accepts it. The
/// `backup` verbs are the case that matters: they read archives and directories
/// rather than serving one request, so they bind no tool and would otherwise be
/// invisible to anything that reads the catalog.
///
/// A command is listed when it binds no tool, and nothing else is listed. A
/// group prefix therefore appears whenever the prefix itself binds nothing, and
/// each of its subcommands appears only when that subcommand binds nothing too:
/// `backup` and `backup create` are both here, and `scratchpad write` is not.
/// That is the same rule [`group_help`] applies when it prints "(no MCP tool)"
/// beside a subcommand with no binding.
pub(crate) fn commands_without_tools() -> Vec<String> {
    let mut unbound: Vec<String> = known_commands()
        .into_iter()
        .filter(|verb| bound_tool(verb).is_none())
        .map(str::to_owned)
        .collect();
    for (group, subcommands) in shared_groups() {
        for sub in subcommands {
            let key = format!("{group} {sub}");
            if bound_tool(&key).is_none() {
                unbound.push(key);
            }
        }
    }
    unbound.sort();
    unbound.dedup();
    unbound
}

/// Subcommand usage for a command's `--help`, with the MCP tool it binds.
///
/// Returns `None` for a command this surface does not own, so an unknown
/// command's `--help` still fails as an unknown command rather than printing help.
pub(crate) fn verb_help(verb: &str) -> Option<(String, &'static str)> {
    let tool = bound_tool(verb)?;
    let usage = crate::cli_help_usage::usage_for(verb)?;
    Some((usage, tool))
}

/// Build the machine-readable CLI catalog from the published MCP schema pack.
///
/// The `tools` array is the pack's tool list verbatim (each entry gains a `tool`
/// field equal to its `name`), so the CLI catalog and the MCP schema cannot drift.
/// The `commands` array is the binding table verbatim. Both stay derived from
/// the MCP side alone, so neither the schema pack nor the binding table can be
/// read as having drifted because a command the CLI accepts binds no tool;
/// `commands_without_tools` is where those commands are listed instead.
pub(crate) fn catalog() -> Value {
    let pack = clio_mcp::schema::schema_pack();
    let tools: Vec<Value> = pack
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    let mut tool = tool.clone();
                    if let Some(name) = tool.get("name").and_then(Value::as_str) {
                        tool["tool"] = Value::String(name.to_owned());
                    }
                    tool
                })
                .collect()
        })
        .unwrap_or_default();
    let commands: Vec<Value> = command_bindings()
        .iter()
        .map(|(command, tool)| json!({ "command": command, "tool": tool }))
        .collect();
    json!({
        "format": "clio_cli_catalog",
        "format_version": 1,
        "mcp_protocol_revision": pack.get("mcp_protocol_revision").cloned().unwrap_or(Value::Null),
        "catalog_hash": pack.get("catalog_hash").cloned().unwrap_or(Value::Null),
        "commands": commands,
        "commands_without_tools": commands_without_tools(),
        "tools": tools,
    })
}

/// Write the catalog as one compact JSON line.
pub(crate) fn write_catalog<W: Write>(w: &mut W) -> io::Result<()> {
    crate::cli_output::write_json(w, &catalog())
}

/// The derived group and prefix lists, attached here because they feed the
/// catalog and the suggestion lookups.
#[path = "cli_help_groups.rs"]
pub(crate) mod groups;

#[cfg(test)]
#[path = "cli_help_dispatch_tests.rs"]
mod dispatch_tests;

#[cfg(test)]
#[path = "cli_help_tests.rs"]
mod tests;
