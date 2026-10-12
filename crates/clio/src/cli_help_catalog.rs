// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `help --json` catalog document's shape:
//! its assembly from the MCP schema pack, the registry records, and the
//! derived group table, with the CLI metadata hash that fingerprints the
//! CLI-side metadata.
//!
//! ## Owns
//! - The catalog document's top-level key set and its one active format
//!   version constant
//! - The serialization of every command record, the shared/global flags, and
//!   the derived groups
//! - `cli_hash`, the FNV-1a fingerprint over the catalog's CLI-side metadata
//! - The fail-closed name validation the assembly runs before emitting
//!
//! ## Does not own
//! - The tool schemas, the protocol revision, or the pack's `catalog_hash`
//!   (see `clio-mcp`)
//! - The binding table and the unbound-command lookup the document reads (see
//!   `cli_help`)
//! - The suggestion lookups and human help rendering (see `cli_help`)
//!
//! ## Boundary
//!
//! Changes to what the catalog document exposes should normally require
//! changes here. Changes to one command's record fields should not.
//!
//! Keep this module focused on the catalog document's shape.

use crate::cli_command_spec::{
    CommandSpec, Example, Flag, Positional, Section, ValueKind, ValueSpec,
};
use serde_json::{Value, json};

/// The catalog document's format version: the one active version, emitted
/// from this constant and no other.
const CATALOG_FORMAT_VERSION: u32 = 2;

/// Build the machine-readable CLI catalog document.
///
/// The `tools` array is the pack's tool list verbatim (each entry gains a
/// `tool` field equal to its `name`), and the pack's protocol revision and
/// `catalog_hash` are carried through unchanged, so the CLI catalog and the
/// MCP schema cannot drift. `commands` is the binding table verbatim;
/// `commands_without_tools` is the unbound command/prefix projection, so a
/// command the CLI accepts that binds no tool is still listed.
/// `command_specs`, `groups`, and `shared_global` serialize the registry
/// records and their derived tables, and `cli_hash` fingerprints that
/// CLI-side metadata — the tools and the pack hash are excluded, because the
/// pack's own hash covers them.
pub(crate) fn catalog() -> Value {
    validate_names();
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
    let commands: Vec<Value> = crate::cli_help::command_bindings()
        .iter()
        .map(|(command, tool)| json!({ "command": command, "tool": tool }))
        .collect();
    let commands_without_tools = crate::cli_help::commands_without_tools();
    let command_specs: Vec<Value> = crate::cli_command_registry::all()
        .map(command_spec_json)
        .collect();
    let groups: Vec<Value> = crate::cli_help::all_groups()
        .map(|(prefix, subs, shared)| {
            json!({ "prefix": prefix, "subcommands": subs, "shared": shared })
        })
        .collect();
    let shared_global = shared_global_json();
    let cli_hash = metadata_hash(&json!({
        "command_specs": command_specs,
        "commands": commands,
        "commands_without_tools": commands_without_tools,
        "groups": groups,
        "shared_global": shared_global,
    }));
    json!({
        "format": "clio_cli_catalog",
        "format_version": CATALOG_FORMAT_VERSION,
        "mcp_protocol_revision": pack.get("mcp_protocol_revision").cloned().unwrap_or(Value::Null),
        "catalog_hash": pack.get("catalog_hash").cloned().unwrap_or(Value::Null),
        "cli_hash": cli_hash,
        "commands": commands,
        "commands_without_tools": commands_without_tools,
        "command_specs": command_specs,
        "groups": groups,
        "shared_global": shared_global,
        "tools": tools,
    })
}

/// One command record as the document serializes it: every field the record
/// model declares, in declaration order, so a reader never re-resolves static
/// data.
fn command_spec_json(spec: &CommandSpec) -> Value {
    json!({
        "name": spec.name,
        "aliases": spec.aliases,
        "summary": spec.summary,
        "notes": spec.notes,
        "positionals": spec.positionals.iter().map(positional_json).collect::<Vec<_>>(),
        "flags": spec.documented.iter().map(flag_json).collect::<Vec<_>>(),
        "accepted_extra": spec.accepted_extra,
        "flag_aliases": spec
            .flag_aliases
            .iter()
            .map(|(alias, canonical)| json!({ "alias": alias, "canonical": canonical }))
            .collect::<Vec<_>>(),
        "usage_override": spec.usage_override,
        "usage_alternates": spec.usage_alternates,
        "examples": spec.examples.iter().map(example_json).collect::<Vec<_>>(),
        "tool": spec.tool,
        "section": section_json(spec.section),
    })
}

/// One flag with its resolved value data, so a reader never re-resolves it.
fn flag_json(flag: &Flag) -> Value {
    json!({
        "long": flag.long,
        "short": flag.short,
        "value": flag.value.map(value_json),
        "default": flag.default,
        "required": flag.required,
        "repeatable": flag.repeatable,
        "description": flag.description,
        "placeholder_override": flag.placeholder_override,
        "value_optional": flag.value_optional,
        "exclusive_group": flag.exclusive_group,
    })
}

/// One flag value: its placeholder, its kind, and its allowed values when the
/// kind restricts them. The kind is a tag, never a number, so the hashed
/// bytes carry no floats.
fn value_json(value: ValueSpec) -> Value {
    let kind = value.kind;
    let mut value = json!({
        "placeholder": value.placeholder,
        "kind": value_kind_json(kind),
    });
    if let ValueKind::Enum(allowed) = kind {
        value["allowed"] = json!(allowed);
    }
    value
}

fn value_kind_json(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Text => "text",
        ValueKind::Uint => "uint",
        ValueKind::Float => "float",
        ValueKind::Enum(_) => "enum",
    }
}

fn positional_json(positional: &Positional) -> Value {
    json!({
        "label": positional.label,
        "required": positional.required,
        "description": positional.description,
    })
}

fn example_json(example: &Example) -> Value {
    json!({ "command": example.command, "comment": example.comment })
}

/// How a command affects stored data, as the document spells it.
fn section_json(section: Section) -> &'static str {
    match section {
        Section::Read => "read",
        Section::Write => "write",
        Section::Reserved => "reserved",
    }
}

/// The shared/global flag metadata a reader needs: the declared global flags
/// with their resolved value data, in the order every parser merges them.
fn shared_global_json() -> Value {
    json!({
        "value_flags": crate::cli_command_registry::GLOBAL_VALUE_FLAGS
            .iter()
            .map(flag_json)
            .collect::<Vec<_>>(),
        "bool_flags": crate::cli_command_registry::GLOBAL_BOOL_FLAGS
            .iter()
            .map(flag_json)
            .collect::<Vec<_>>(),
    })
}

/// The CLI metadata hash: FNV-1a 64 over the compact canonical bytes of the
/// catalog's CLI-side metadata — the command specs, the two command
/// projections, the groups, and the shared/global flags. `serde_json` sorts
/// object keys, every array keeps its registry or derived order, and the
/// metadata holds no floats, so the compact bytes are canonical. The pack's
/// `catalog_hash` covers the tools and the protocol revision; the distinct
/// `cli_hash` name keeps the two payloads apart.
fn metadata_hash(payload: &Value) -> String {
    let bytes = serde_json::to_string(payload).unwrap_or_default();
    clio_mcp::schema::fnv1a64_hex(bytes.as_bytes())
}

/// Validate the names the document is about to emit, naming the offender and
/// failing closed the way the binding accessor does.
///
/// The registry's static-data contract runs over the whole record set against
/// the catalog spine, so a binding missing its leaf or its tool fails before
/// anything is emitted. The derived groups and the unbound-command list may
/// name only commands `clio help <topic>` reaches: a name the CLI refuses
/// would advertise a command the dispatcher rejects.
fn validate_names() {
    let specs: Vec<&CommandSpec> = crate::cli_command_registry::all().collect();
    if let Err(error) =
        crate::cli_command_registry::validate(&specs, crate::cli_command_bindings::CATALOG_ORDER)
    {
        panic!("the CLI catalog's records do not validate: {error}");
    }
    for (prefix, subs, _) in crate::cli_help::all_groups() {
        assert!(
            crate::cli_help::is_known_command(prefix),
            "the catalog's group prefix `{prefix}` is not a known command"
        );
        for sub in subs {
            let key = format!("{prefix} {sub}");
            assert!(
                crate::cli_help::is_known_command(&key),
                "the catalog's group entry `{key}` is not a known command"
            );
        }
    }
    for command in crate::cli_help::commands_without_tools() {
        assert!(
            crate::cli_help::is_known_command(&command),
            "the catalog's unbound command `{command}` is not a known command"
        );
    }
}

#[cfg(test)]
#[path = "cli_help_catalog_tests.rs"]
mod tests;
