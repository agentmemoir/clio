// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI help catalog and command suggestion: catalog/tool
//! name parity with the MCP schema pack, verb bindings, and `did you mean`.
//!
//! ## Owns
//! - Assertions over `catalog`, `command_bindings`, and `suggest_command`
//!
//! ## Does not own
//! - The catalog implementation under test (`cli_help`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::collections::BTreeSet;

#[test]
fn catalog_tool_names_equal_the_schema_pack_names() {
    let catalog = catalog();
    let catalog_names: BTreeSet<String> = catalog["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name").to_owned())
        .collect();
    let pack_names: BTreeSet<String> = clio_mcp::schema::catalog_defs()
        .iter()
        .map(|def| def.name.to_owned())
        .collect();
    assert_eq!(catalog_names, pack_names);
}

#[test]
fn every_catalog_entry_names_the_tool_it_binds() {
    for tool in catalog()["tools"].as_array().expect("tools") {
        let name = tool["name"].as_str().expect("name");
        assert_eq!(tool["tool"].as_str(), Some(name));
    }
}

#[test]
fn catalog_commands_bind_read_verbs_to_tools() {
    let commands = catalog()["commands"].as_array().expect("commands").clone();
    for (command, tool) in command_bindings() {
        let found = commands.iter().any(|entry| {
            entry["command"].as_str() == Some(command) && entry["tool"].as_str() == Some(tool)
        });
        assert!(found, "missing binding {command} -> {tool}");
    }
}

#[test]
fn known_commands_cover_reserved_and_read_verbs() {
    let known = known_commands();
    for verb in RESERVED_VERBS.iter().chain(READ_VERBS.iter()) {
        assert!(known.contains(verb), "missing verb {verb}");
    }
}

/// The `backup` verbs bind no MCP tool, so the binding table alone cannot list
/// them. They are listed in the catalog's own `commands_without_tools` array,
/// and the binding-derived `commands` array is left exactly as it was.
#[test]
fn the_catalog_lists_the_commands_that_bind_no_mcp_tool() {
    let catalog = catalog();
    let unbound: Vec<String> = catalog["commands_without_tools"]
        .as_array()
        .expect("commands_without_tools is an array")
        .iter()
        .map(|entry| entry.as_str().expect("a command name").to_owned())
        .collect();

    for verb in [
        "backup",
        "backup create",
        "backup dir",
        "backup list",
        "backup prune",
        "backup restore",
        "backup verify",
        "install",
        "uninstall",
    ] {
        assert!(
            unbound.contains(&verb.to_owned()),
            "missing {verb}: {unbound:?}"
        );
    }
    // Every entry really is unbound, and the order is sorted so two runs emit
    // the same bytes.
    let mut sorted = unbound.clone();
    sorted.sort();
    assert_eq!(unbound, sorted, "the array is emitted in a stable order");
    for command in &unbound {
        assert!(
            !command_bindings().iter().any(|(name, _)| name == command),
            "{command} does bind a tool"
        );
    }

    // The binding table is unchanged: `commands` is still that table, entry for
    // entry and in the same order, so the catalog contract the other tests
    // rely on cannot have moved.
    let commands = catalog["commands"]
        .as_array()
        .expect("commands array")
        .clone();
    assert_eq!(commands.len(), command_bindings().len());
    for (entry, (command, tool)) in commands.iter().zip(command_bindings()) {
        assert_eq!(entry["command"].as_str(), Some(command));
        assert_eq!(entry["tool"].as_str(), Some(tool));
    }

    // Every command this surface knows is in one of the two arrays, so a verb
    // the CLI accepts can never be missing from the catalog.
    let listed: BTreeSet<&str> = commands
        .iter()
        .map(|entry| entry["command"].as_str().expect("a command name"))
        .chain(unbound.iter().map(String::as_str))
        .collect();
    for verb in known_commands() {
        assert!(listed.contains(verb), "missing verb {verb}");
    }
}

#[test]
fn suggest_command_finds_typos_and_exact_matches() {
    assert_eq!(suggest_command("recal"), Some("recall"));
    assert_eq!(suggest_command("mcp"), Some("mcp"));
    assert_eq!(suggest_command("zzzzzz"), None);
}

#[test]
fn verb_help_names_each_read_verb_and_its_tool() {
    for (verb, tool) in command_bindings() {
        let (usage, bound) = verb_help(verb).unwrap_or_else(|| panic!("{verb}"));
        assert!(usage.starts_with("usage: clio "), "{verb}: {usage}");
        assert_eq!(bound, tool, "{verb}");
    }
    assert!(verb_help("bogus").is_none());
}

#[test]
fn verb_usage_lines_keep_the_canonical_output_spelling() {
    // Decision: per-verb usage lines keep `[--output json|text]`; the `-o`
    // alias is a global flag documented once in the top-level help. This pin
    // fails if a partial find/replace half-updates the surface.
    for verb in ["recall", "get", "inspect", "stats", "summarize", "remember"] {
        let usage = crate::cli_help_usage::usage_for(verb).unwrap_or_else(|| panic!("{verb}"));
        assert!(usage.contains("[--output json|text]"), "{verb}: {usage}");
        assert!(!usage.contains("-o json|text"), "{verb}: {usage}");
        assert!(!usage.contains("-o|--output"), "{verb}: {usage}");
    }
}

#[test]
fn group_subcommands_and_help_are_consistent() {
    for cmd in [
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
        "hygiene",
    ] {
        let subs = group_subcommands(cmd).unwrap_or_else(|| panic!("{cmd}"));
        assert!(!subs.is_empty(), "{cmd}");
        let help = group_help(cmd, subs);
        assert!(help.contains(&format!("clio {cmd} <subcommand>")), "{help}");
        for sub in subs {
            let key = format!("{cmd} {sub}");
            assert!(help.contains(&key), "{help}");
            assert!(bound_tool(&key).is_some(), "{key}");
        }
    }
    assert!(group_subcommands("bogus").is_none());
}

#[test]
fn write_catalog_emits_parseable_json() {
    let mut out = Vec::new();
    write_catalog(&mut out).expect("write");
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("catalog is valid json");
    assert_eq!(parsed["format"], serde_json::json!("clio_cli_catalog"));
}

/// `canonical put` accepts the same optional bounded `--context` as the MCP
/// `canonical_put` schema, so the usage line must document it.
#[test]
fn canonical_put_usage_documents_context() {
    let (usage, _) = verb_help("canonical put").expect("canonical put usage");
    assert!(usage.contains("[--context C]"), "{usage}");
}
