// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the seam between the dispatcher and the registry, by a
//! targeted scan of the production sources: every dispatch arm names a command
//! the registry holds, every registered command is reachable from a dispatch
//! arm, every registered sub-command is routed by a match arm or equality test
//! of its group's dispatcher or recorded with the verb-table row or fallback
//! handler declaration the scan pins for it, and the duplicate row sources
//! this phase deleted stay deleted.
//!
//! ## Owns
//! - The dispatcher completeness scan over `main.rs`'s `dispatch` arms and the
//!   group dispatchers' sub-verb arms, in both directions, with no code
//!   generation
//! - The duplicate-source tripwire over the usage, catalog, binding, group, and
//!   backup modules
//!
//! ## Does not own
//! - The dispatch itself (`main`) or the registry lookups
//!   (`cli_command_registry`)
//! - The parser-parity and literal-read scans (`cli_help_usage_tests`,
//!   `flag_read_scan_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the dispatcher-to-registry seam.

use std::path::Path;

/// The command arms of `dispatch`, read from the production source.
///
/// The match's string literals are the arms; a verb-shaped literal (lowercase
/// letters, digits, and dashes, no leading dash) is a command name. The flag
/// spellings (`--version`, `-V`, `--help`, `-h`) are routes to the same
/// commands, not commands, and the shape filter leaves them out.
fn dispatch_arms() -> Vec<String> {
    let source = production_source("src/main.rs");
    let start = source
        .find("fn dispatch")
        .expect("`main` declares `dispatch`");
    let end = source[start..]
        .find("\n#[cfg(test)]")
        .expect("`dispatch` is the last item before the test modules");
    let body = &source[start..start + end];
    let mut arms: Vec<String> = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(part) = chars.next() {
        if part != '"' {
            continue;
        }
        let mut literal = String::new();
        for inner in chars.by_ref() {
            match inner {
                '"' => break,
                other => literal.push(other),
            }
        }
        let verb_shaped = !literal.is_empty()
            && !literal.starts_with('-')
            && literal
                .chars()
                .all(|part| part.is_ascii_lowercase() || part.is_ascii_digit() || part == '-');
        if verb_shaped && !arms.contains(&literal) {
            arms.push(literal);
        }
    }
    arms
}

/// One production file's source, resolved from the manifest directory.
fn production_source(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("the scan cannot read {path}: {error}"))
}

/// Every dispatch arm names a command the registry holds: a record's own name
/// or a group prefix some record extends.
///
/// An arm naming no record would route a command the help surfaces refuse, so
/// the catalog and the dispatch would disagree about what the CLI runs.
#[test]
fn every_dispatch_arm_names_a_registered_command() {
    for arm in dispatch_arms() {
        let prefixed = format!("{arm} ");
        assert!(
            crate::cli_command_registry::find(&arm).is_some()
                || crate::cli_command_registry::all().any(|spec| spec.name.starts_with(&prefixed)),
            "dispatch arm `{arm}` names no record and no group prefix"
        );
    }
}

/// Every registered command is reachable from a dispatch arm: a single-token
/// record names itself, a multi-token record's group prefix is the arm.
///
/// The direction the other check cannot see: a record whose group prefix lost
/// its arm would register a command the dispatcher refuses, and `clio help`
/// would advertise it.
#[test]
fn every_registered_command_is_reachable_from_a_dispatch_arm() {
    let arms = dispatch_arms();
    for spec in crate::cli_command_registry::all() {
        let arm = match spec.name.split_once(' ') {
            Some((prefix, _)) => prefix,
            None => spec.name,
        };
        assert!(
            arms.iter().any(|candidate| candidate == arm),
            "`{}` registers under the arm `{arm}`, which dispatch does not take",
            spec.name
        );
    }
}

/// The duplicate row sources this phase deleted stay deleted.
///
/// Each needle is the identifier of a table whose rows duplicated the registry
/// records: the usage rows, the catalog fallback rows, the static bindings,
/// the hand-written group tables, and the five backup leaves' flag tables. Their
/// return would put a second source behind rows the records already carry, so
/// the scan fails on any of them in the production modules they lived in.
/// Comment lines are skipped, so a doc mention is not a source.
#[test]
fn the_deleted_duplicate_row_sources_stay_deleted() {
    const RETURNED_SOURCES: &[(&str, &str)] = &[
        ("src/cli_help_usage.rs", "VERB_USAGE"),
        ("src/command_catalog.rs", "ENTRIES_BEFORE"),
        ("src/command_catalog.rs", "ENTRIES_AFTER"),
        ("src/cli_command_bindings.rs", "STATIC_BINDINGS"),
        ("src/cli_help.rs", "GROUP_SUBCOMMANDS"),
        ("src/cli_help.rs", "RESERVED_SUBCOMMANDS"),
        ("src/cli_help.rs", "READ_VERBS"),
        ("src/cli_help.rs", "WRITE_VERBS"),
        ("src/cli_help.rs", "RESERVED_VERBS"),
        ("src/backup_create_cli.rs", "VALUE_FLAGS"),
        ("src/backup_create_cli.rs", "BOOL_FLAGS"),
        ("src/backup_list_cli.rs", "VALUE_FLAGS"),
        ("src/backup_prune_cli.rs", "VALUE_FLAGS"),
        ("src/backup_prune_cli.rs", "BOOL_FLAGS"),
        ("src/backup_restore_cli.rs", "VALUE_FLAGS"),
        ("src/backup_restore_cli.rs", "BOOL_FLAGS"),
        ("src/backup_verify_cli.rs", "VALUE_FLAGS"),
        ("src/backup_verify_cli.rs", "BOOL_FLAGS"),
    ];
    let mut sources = std::collections::BTreeMap::new();
    for (path, _) in RETURNED_SOURCES {
        sources
            .entry(*path)
            .or_insert_with(|| production_source(path));
    }
    for (path, needle) in RETURNED_SOURCES {
        let source = &sources[path];
        let returned = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .any(|line| line.contains(needle));
        assert!(
            !returned,
            "`{needle}` is back in {path}: a duplicate row source returned, and \
             the registry records are the one source for those rows"
        );
    }
}

/// The production modules that dispatch each group's sub-verbs: the group
/// prefix a record extends, and a module that routes its leaves.
///
/// A sub-verb is routed when such a module names it in a match arm or an
/// equality test. A group may have several modules (`task` reads in one,
/// writes in another), so rows repeat the prefix.
/// `every_registered_sub_command_has_a_dispatch_arm` pins the map in both
/// directions: an unlisted group fails that test, and a listed group no record
/// extends fails it too.
const GROUP_ARM_SOURCES: &[(&str, &str)] = &[
    ("audit", "src/cli_read_workspace.rs"),
    ("auth", "src/auth_cli.rs"),
    ("backup", "src/backup_cli.rs"),
    ("belief", "src/cli_read_belief.rs"),
    ("belief", "src/cli_write_graph.rs"),
    ("canonical", "src/cli_write_workspace.rs"),
    ("compose", "src/compose_cli.rs"),
    ("config", "src/config_cli.rs"),
    ("failure", "src/cli_read_history.rs"),
    ("failure", "src/cli_write_persona.rs"),
    ("graph", "src/cli_read_graph.rs"),
    ("graph", "src/cli_write_graph.rs"),
    ("history", "src/cli_read_history.rs"),
    ("hygiene", "src/cli_read_hygiene.rs"),
    ("hygiene", "src/cli_write_hygiene.rs"),
    ("intent", "src/cli_read_workspace.rs"),
    ("maintenance", "src/cli_read_graph.rs"),
    ("mcp", "src/mcp_cli.rs"),
    ("memtree", "src/cli_read_history.rs"),
    ("ops", "src/ops_cli.rs"),
    ("persona", "src/cli_read_graph.rs"),
    ("persona", "src/cli_write_persona.rs"),
    ("ranking", "src/ranking_cli.rs"),
    ("retention", "src/retention_cli.rs"),
    ("scratchpad", "src/cli_write_workspace.rs"),
    ("shared", "src/cli_write_shared.rs"),
    ("sync", "src/sync_cli.rs"),
    ("sync", "src/sync_key_cli.rs"),
    ("task", "src/cli_read_history.rs"),
    ("task", "src/cli_write_persona.rs"),
    ("triple", "src/cli_read_belief.rs"),
    ("triple", "src/cli_write_graph.rs"),
];

/// The registered sub-verbs whose group declares them in the module's own
/// `&[...]` verb table and routes them through an arm-free fallback: the record
/// and the module that declares it.
///
/// `every_registered_sub_command_has_a_dispatch_arm` pins these entries, so a
/// deleted table row fails even though the group never had an arm of its own
/// for the verb.
const TABLE_SUB_VERBS: &[(&str, &str)] = &[
    ("belief history", "src/cli_read_belief.rs"),
    ("canonical get", "src/cli_write_workspace.rs"),
    ("failure record", "src/cli_write_persona.rs"),
    ("graph link", "src/cli_write_graph.rs"),
    ("history temporal", "src/cli_read_history.rs"),
    ("hygiene clean", "src/cli_write_hygiene.rs"),
    ("hygiene log", "src/cli_read_hygiene.rs"),
    ("maintenance status", "src/cli_read_graph.rs"),
];

/// The registered sub-verbs their group reaches through its own `_` fallback
/// arm, whose accepted set comes from the registry record rather than from a
/// table in the module: the record, its group's module, and the handler the
/// fallback calls.
///
/// The scan pins the handler's declaration for these three, because the
/// dispatcher has no literal arm or table entry of theirs to pin.
const FALLBACK_SUB_VERBS: &[(&str, &str, &str)] = &[
    ("auth token revoke", "src/auth_cli.rs", "run_revoke"),
    ("config use", "src/config_cli.rs", "use_call"),
    ("ranking set", "src/ranking_cli.rs", "set_call"),
];

/// Whether `source` routes the sub-verb `needle`: a non-comment line naming it
/// quoted in a match arm or an equality test.
///
/// A line naming the sub-verb as prose, as an argument label
/// (`one_positional(parsed, "audit trail", ...)`) or as an entry of a hint or
/// verb table carries no route marker and is not a route, so the scan fails
/// when the arm that carried it is gone even though other text still names it.
fn routes_sub_verb(source: &str, needle: &str) -> bool {
    let quoted = format!("\"{needle}\"");
    source.lines().any(|line| {
        !line.trim_start().starts_with("//")
            && line.contains(&quoted)
            && (line.contains("=>") || line.contains("=="))
    })
}

/// Whether `source` declares the sub-verb `needle` in one of its own `&[...]`
/// verb tables: a line naming it in the table, or a listed entry before the
/// table closes.
fn table_declares_sub_verb(source: &str, needle: &str) -> bool {
    let quoted = format!("\"{needle}\"");
    let mut in_table = false;
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if (in_table || line.contains("&[")) && line.contains(&quoted) {
            return true;
        }
        in_table = (in_table || line.contains("&[")) && !line.contains(']');
    }
    false
}

/// Every registered sub-command is routed by an arm of its group's dispatcher,
/// or recorded above with the declaration that keeps it reachable. A removed
/// sub-arm fails here even though the group's prefix arm still stands, because
/// `dispatch` routes groups wholesale and the prefix check cannot see a
/// missing leaf.
///
/// The scan reads the production text at run time, the same way the
/// duplicate-source tripwire does, so it adds no compile-time dependency on
/// the modules it reads.
#[test]
fn every_registered_sub_command_has_a_dispatch_arm() {
    let mut sources = std::collections::BTreeMap::new();
    for (_, path) in GROUP_ARM_SOURCES {
        sources
            .entry(*path)
            .or_insert_with(|| production_source(path));
    }
    let mut unrouted: Vec<&str> = Vec::new();
    for spec in crate::cli_command_registry::all() {
        let Some((prefix, sub)) = spec.name.split_once(' ') else {
            continue;
        };
        let mut mapped = false;
        let mut routed = false;
        for (group, path) in GROUP_ARM_SOURCES {
            if *group != prefix {
                continue;
            }
            mapped = true;
            let source = &sources[path];
            if routes_sub_verb(source, spec.name) || routes_sub_verb(source, sub) {
                routed = true;
                break;
            }
        }
        assert!(
            mapped,
            "`{}` extends the group `{prefix}`, which `GROUP_ARM_SOURCES` does \
             not map to a dispatcher",
            spec.name
        );
        if !routed {
            unrouted.push(spec.name);
        }
    }
    let mut expected: Vec<&str> = TABLE_SUB_VERBS
        .iter()
        .map(|(name, _)| *name)
        .chain(FALLBACK_SUB_VERBS.iter().map(|(name, _, _)| *name))
        .collect();
    unrouted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        unrouted, expected,
        "a registered sub-command has no arm in its group's dispatcher, and no \
         recorded table row or fallback route reaches it"
    );
    for (name, path) in TABLE_SUB_VERBS {
        assert!(
            table_declares_sub_verb(&sources[path], name),
            "`{name}` has no arm in {path}, and that module's verb table no \
             longer declares it"
        );
    }
    for (name, path, handler) in FALLBACK_SUB_VERBS {
        let declaration = format!("fn {handler}(");
        assert!(
            sources[path].contains(&declaration),
            "`{name}` routes through `{handler}`, which {path} no longer \
             declares as `{declaration}`"
        );
    }
    for (group, path) in GROUP_ARM_SOURCES {
        assert!(
            crate::cli_command_registry::all().any(|spec| matches!(
                spec.name.split_once(' '),
                Some((prefix, _)) if prefix == *group
            )),
            "`GROUP_ARM_SOURCES` routes `{group}` from {path}, but no registered \
             record extends that group"
        );
    }
}
