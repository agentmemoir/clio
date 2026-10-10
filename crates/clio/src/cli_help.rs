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
//! - The CLI command → MCP tool binding table (including two-token groups)
//! - `help --json` catalog assembly from `clio_mcp::schema::schema_pack()`
//! - The commands the CLI accepts that bind no MCP tool, and so are listed in
//!   the catalog separately from the binding table
//! - Unknown-command/subcommand suggestion and per-command usage lookup
//!
//! ## Does not own
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

/// Top-level verbs owned by other surfaces; new verbs must not collide.
pub(crate) const RESERVED_VERBS: &[&str] = &[
    "version",
    "health",
    "help",
    "mcp",
    "ops",
    "retention",
    "compose",
    "setup",
    "status",
    "config",
    "ranking",
    "sync",
    "backup",
    "auth",
    "install",
    "uninstall",
];

/// Read commands this surface binds (core verbs, group prefixes, and singles).
pub(crate) const READ_VERBS: &[&str] = &[
    "recall",
    "get",
    "show",
    "inspect",
    "stats",
    "task",
    "failure",
    "memtree",
    "history",
    "triple",
    "belief",
    "graph",
    "persona",
    "intent",
    "maintenance",
    "audit",
    "associations",
    "compose-context",
    "scratchpad",
    "canonical",
    "shared",
    "hygiene",
];

/// Write commands this surface binds (top-level verbs and group subcommands).
pub(crate) const WRITE_VERBS: &[&str] = &[
    "remember",
    "admit",
    "summarize",
    "consolidate",
    "update",
    "invalidate",
    "discard",
    "correct",
    "validate",
    "batch",
    "export",
    "import",
    "erase",
];

/// Two-token group commands and their subcommands (read + write).
const GROUP_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("task", &["get", "history", "upsert"]),
    ("failure", &["list", "record"]),
    ("memtree", &["query", "get"]),
    ("history", &["temporal"]),
    ("triple", &["query", "add", "end"]),
    ("belief", &["history", "observe"]),
    ("graph", &["query", "link"]),
    ("persona", &["get", "stable", "observe"]),
    ("intent", &["gate"]),
    ("maintenance", &["status"]),
    ("audit", &["trail"]),
    ("scratchpad", &["write", "read", "clear"]),
    ("canonical", &["put", "get"]),
    ("shared", &["store", "retrieve", "discard"]),
    ("hygiene", &["audit", "clean", "log"]),
    ("config", &["get", "set", "profiles", "use"]),
    ("ranking", &["get", "set"]),
    (
        "sync",
        &[
            "status",
            "push",
            "pull",
            "serve",
            "keygen",
            "key export",
            "key status",
        ],
    ),
    (
        "backup",
        &["create", "dir", "list", "prune", "restore", "verify"],
    ),
    ("auth", &["token"]),
];

/// The subcommands of a two-token group command, when `cmd` is a group.
pub(crate) fn group_subcommands(cmd: &str) -> Option<&'static [&'static str]> {
    GROUP_SUBCOMMANDS
        .iter()
        .find(|(name, _)| *name == cmd)
        .map(|(_, subs)| *subs)
}

/// Every known top-level command (reserved + read + write).
pub(crate) fn known_commands() -> Vec<&'static str> {
    let mut all = RESERVED_VERBS.to_vec();
    all.extend_from_slice(READ_VERBS);
    all.extend_from_slice(WRITE_VERBS);
    all
}

/// The CLI command → MCP tool binding, so a command never silently drifts from
/// its tool. Two-token commands use a space-separated key.
pub(crate) fn command_bindings() -> &'static [(&'static str, &'static str)] {
    &[
        ("recall", "retrieve"),
        ("get", "get"),
        ("show", "get"),
        ("inspect", "inspect"),
        ("stats", "stats"),
        ("remember", "store"),
        ("admit", "admit_preview"),
        ("summarize", "summarize"),
        ("consolidate", "consolidate"),
        ("update", "update"),
        ("invalidate", "invalidate"),
        ("discard", "discard"),
        ("correct", "correct"),
        ("task get", "task_get"),
        ("task history", "task_history"),
        ("failure list", "failures_for_task"),
        ("memtree query", "memtree_query"),
        ("memtree get", "memtree_get"),
        ("history temporal", "temporal_history"),
        ("triple query", "triple_query"),
        ("triple add", "triple_add"),
        ("triple end", "triple_end"),
        ("belief history", "belief_history"),
        ("belief observe", "belief_observe"),
        ("graph query", "graph_query"),
        ("graph link", "graph_link"),
        ("associations", "associations"),
        ("persona get", "persona_get"),
        ("persona stable", "persona_put_stable"),
        ("persona observe", "persona_observe_preference"),
        ("task upsert", "task_upsert"),
        ("failure record", "failure_record"),
        ("scratchpad write", "scratchpad_write"),
        ("scratchpad read", "scratchpad_read"),
        ("scratchpad clear", "scratchpad_clear"),
        ("canonical put", "canonical_put"),
        ("canonical get", "canonical_get"),
        ("shared store", "shared_store"),
        ("shared retrieve", "shared_retrieve"),
        ("shared discard", "shared_discard"),
        ("validate", "validate"),
        ("batch", "batch"),
        ("maintenance status", "maintenance_status"),
        ("intent gate", "intent_gate"),
        ("compose-context", "compose_context"),
        ("audit trail", "audit_trail"),
        ("hygiene audit", "hygiene_audit"),
        ("hygiene clean", "hygiene_clean"),
        ("hygiene log", "hygiene_log_list"),
        ("export", "export"),
        ("import", "import"),
        ("erase", "erase_request"),
        ("config get", "config_get"),
        ("config set", "config_set"),
        ("config profiles", "config_profiles"),
        ("config use", "config_profile_apply"),
        ("ranking get", "ranking_env_get"),
        ("ranking set", "ranking_env_set"),
        ("sync status", "sync_status"),
        ("sync push", "sync_push"),
        ("sync pull", "sync_pull"),
        ("sync serve", "sync_serve"),
        ("sync key status", "sync_key_status"),
    ]
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
        // `auth token` is a group whose subcommand is itself a group, so its three
        // verbs are named directly. `sync key export` and `sync key status` are
        // three-token commands whose two-word subcommand sits in the group table.
        [sub, verb2] => {
            (*sub == "token" && verb == "auth" && matches!(*verb2, "add" | "list" | "revoke"))
                || subcommands_of(verb).is_some_and(|subcommands| {
                    subcommands
                        .iter()
                        .any(|name| name.split_once(' ') == Some((*sub, *verb2)))
                })
        }
        _ => false,
    }
}

/// Every command the dispatcher will run, as the full name a reader would type.
///
/// A verb that owns subcommands is represented by its subcommands, not by itself,
/// because that is how the catalog reads: `clio ops reindex` is a command and
/// `clio ops` is only a prefix. This is the one list a completeness check can
/// compare the catalog against, so a command nobody remembered to write a catalog
/// entry for is a name this function returns and the catalog does not.
///
/// It is the same three shapes `is_known_command` accepts, built from the same
/// three tables, so the two cannot disagree about what a command is.
#[cfg(test)]
pub(crate) fn all_known_commands() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = Vec::new();
    for verb in known_commands() {
        match subcommands_of(verb) {
            Some(subs) => all.extend(subs.iter().map(|sub| leak_name(verb, sub))),
            None => all.push(verb),
        }
    }
    // `auth token` is the one subcommand that is itself a group, so the two-token
    // name is a prefix rather than a command and its three verbs are named here.
    all.retain(|name| *name != "auth token");
    for verb in ["add", "list", "revoke"] {
        all.push(leak_name("auth token", verb));
    }
    all
}

/// The subcommands of `verb`, from whichever of the two tables declares it.
fn subcommands_of(verb: &str) -> Option<&'static [&'static str]> {
    group_subcommands(verb).or_else(|| {
        RESERVED_SUBCOMMANDS
            .iter()
            .find(|(reserved, _)| *reserved == verb)
            .map(|(_, subs)| *subs)
    })
}

/// The name of a two-token command, kept for the life of the process.
///
///
/// The command tables are static, so this runs once per process at most, and a
/// `&'static str` is what the catalog and its tests both want to compare against.
#[cfg(test)]
fn leak_name(verb: &str, sub: &str) -> &'static str {
    Box::leak(format!("{verb} {sub}").into_boxed_str())
}

/// The subcommands of the reserved verbs, which bind no MCP tool and are not
/// group commands the dispatcher resolves.
///
/// `cli help mcp http` and `clio ops repair` are both real commands, and neither
/// is reachable through the group table, so the topic check needs them named.
const RESERVED_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("mcp", &["schema-export", "stdio", "http"]),
    (
        "ops",
        &["diagnose", "verify", "doctor", "repair", "reindex"],
    ),
    ("compose", &["up", "down"]),
    ("retention", &["schema", "get", "set"]),
];

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
    for (group, subcommands) in GROUP_SUBCOMMANDS {
        for sub in *subcommands {
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
pub(crate) fn verb_help(verb: &str) -> Option<(&'static str, &'static str)> {
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

#[cfg(test)]
#[path = "cli_help_tests.rs"]
mod tests;
