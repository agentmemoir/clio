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

/// `known_commands` is the set of top-level verbs the registry records spell:
/// every single-token record names itself, every multi-token record extends
/// its prefix, and nothing else is listed.
///
/// The two directions are the independent inputs: a verb listed with no record
/// would suggest a command the dispatcher refuses, and a record whose verb is
/// missing would hide a command the CLI runs from the suggestion lookup.
#[test]
fn known_commands_are_the_registrys_top_level_verbs() {
    let verbs = known_commands();
    for verb in &verbs {
        let prefixed = format!("{verb} ");
        assert!(
            crate::cli_command_registry::find(verb).is_some()
                || crate::cli_command_registry::all().any(|spec| spec.name.starts_with(&prefixed)),
            "`{verb}` is a known command with no record"
        );
    }
    for spec in crate::cli_command_registry::all() {
        let verb = match spec.name.split_once(' ') {
            Some((prefix, _)) => prefix,
            None => spec.name,
        };
        assert!(
            verbs.contains(&verb),
            "`{verb}` has records but is not a known command"
        );
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
        // The reserved surfaces' groups and the intermediate `auth token`
        // group bind no tool either, and their subcommands are the rows the
        // unbound array used to omit.
        "mcp schema-export",
        "mcp stdio",
        "mcp http",
        "ops diagnose",
        "ops verify",
        "ops doctor",
        "ops repair",
        "ops reindex",
        "compose up",
        "compose down",
        "retention schema",
        "retention get",
        "retention set",
        "auth token add",
        "auth token list",
        "auth token revoke",
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

    // Every command this surface knows is in one of the two arrays, so a
    // command the CLI accepts can never be missing from the catalog. The
    // record sweep is the independent source: a record the group table failed
    // to derive still fails here, which is what the sixteen omitted
    // subcommands needed. The prefix sweep covers what has no record of its
    // own.
    let listed: BTreeSet<&str> = commands
        .iter()
        .map(|entry| entry["command"].as_str().expect("a command name"))
        .chain(unbound.iter().map(String::as_str))
        .collect();
    for spec in crate::cli_command_registry::all() {
        assert!(listed.contains(spec.name), "missing command {}", spec.name);
    }
    for verb in known_commands() {
        assert!(listed.contains(verb), "missing verb {verb}");
    }
    for (prefix, _, _) in all_groups() {
        assert!(listed.contains(prefix), "missing group prefix {prefix}");
    }
}

#[test]
fn suggest_command_finds_typos_and_exact_matches() {
    assert_eq!(suggest_command("recal"), Some("recall"));
    assert_eq!(suggest_command("mcp"), Some("mcp"));
    assert_eq!(suggest_command("zzzzzz"), None);
}

/// `suggest` keeps the earlier candidate on a tie, so the hint a reader sees
/// is a function of the authored `KNOWN_VERB_ORDER` and not of the registry's
/// contents alone. This pins the ties that order decides, so a mechanical
/// re-derivation cannot move them silently.
#[test]
fn suggest_command_ties_resolve_by_the_authored_order() {
    // `s` scores `sync` and `show` equally; `sync` is the earlier entry.
    assert_eq!(suggest_command("s"), Some("sync"));
    // `st` scores `setup` and `stats` equally; `setup` is the earlier entry.
    assert_eq!(suggest_command("st"), Some("setup"));
    // `i` scores `intent` and `import` equally; `intent` is the earlier entry.
    assert_eq!(suggest_command("i"), Some("intent"));
    // The other member of the `s` tie is reached by its own input, so the
    // tie is decided by the order and not by reachability.
    assert_eq!(suggest_command("sh"), Some("show"));

    // The differential, executable: the registry's first-occurrence verb
    // order resolves the same `s` tie to `show` instead.
    let mut registry_order: Vec<&'static str> = Vec::new();
    for spec in crate::cli_command_registry::all() {
        let verb = match spec.name.split_once(' ') {
            Some((prefix, _)) => prefix,
            None => spec.name,
        };
        if !registry_order.contains(&verb) {
            registry_order.push(verb);
        }
    }
    assert_eq!(suggest("s", &registry_order), Some("show"));
}

/// `is_known_command` names the three depths `clio help <topic>` may take, and
/// a fourth token is not one of them: the first three words here name a real
/// command, so the refusal is about the depth rather than the words.
#[test]
fn is_known_command_refuses_more_than_three_tokens() {
    assert!(is_known_command("auth token add"));
    assert!(!is_known_command("auth token add extra"));
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

/// The catalog's two command arrays are one ordered, duplicate-free list.
///
/// Set membership alone would pass while a duplicate row changed what a machine
/// reader sees. Every listed name must be one `clio help <topic>` reaches, no
/// command may appear twice across the two arrays, and the unbound array must
/// keep its sorted order.
#[test]
fn the_two_catalog_arrays_list_every_command_once_in_order() {
    let catalog = catalog();
    let names = |key: &str| -> Vec<String> {
        catalog[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} must be an array"))
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .or_else(|| entry["command"].as_str())
                    .expect("a command name")
                    .to_owned()
            })
            .collect()
    };
    let bound = names("commands");
    let unbound = names("commands_without_tools");

    let mut listed: Vec<&str> = Vec::new();
    for command in bound.iter().chain(unbound.iter()) {
        assert!(
            !listed.contains(&command.as_str()),
            "`{command}` is listed twice across the catalog arrays"
        );
        assert!(
            is_known_command(command),
            "`{command}` is listed but `clio help <topic>` does not reach it"
        );
        listed.push(command);
    }
    let mut sorted = unbound.clone();
    sorted.sort();
    assert_eq!(unbound, sorted, "the unbound array must stay sorted");
}
