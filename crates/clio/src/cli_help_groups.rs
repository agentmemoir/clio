// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the command group and prefix lists the help
//! catalog needs: every group's subcommand labels and every top-level verb,
//! derived from the registry records so the dispatcher has no second list to
//! drift.
//!
//! ## Owns
//! - `GROUPS`, the prefix → subcommand-labels table built once from the
//!   records' own names, with the `auth token` intermediate group it derives
//! - `group_subcommands`, the shared groups' subcommand lookup, and
//!   `subcommands_of`, the same lookup over every group including the
//!   reserved surfaces'
//! - `known_commands`, every top-level verb the CLI accepts
//!
//! ## Does not own
//! - The records themselves (see `cli_command_registry`)
//! - The dispatch that routes a group's subcommand (see `main`, `cli_read`,
//!   and the group surfaces)
//! - The top-level help layout (see the binary's `print_help`)
//!
//! ## Boundary
//!
//! Changes to which subcommands a group takes should normally require changes
//! to its records, not here. Changes to how the labels are derived should
//! require changes here.
//!
//! Keep this module focused on the derived group and prefix lists.
//
// The labels are the records' own names minus their prefix, in registry
// order, so the lists read the same as the hand tables they replaced.

use std::sync::LazyLock;

/// The group prefixes the reserved surfaces resolve through their own
/// dispatch: `clio mcp`, `clio ops`, `clio compose`, `clio retention`, and
/// `clio compliance` answer their subcommands from their surface modules, so
/// they are not shared groups. Their subcommand labels still derive from the
/// records.
const SURFACE_OWNED_GROUPS: &[&str] = &["mcp", "ops", "compose", "retention", "compliance"];

/// One derived group: its prefix, its subcommand labels, and what resolves it.
struct Group {
    prefix: &'static str,
    subs: &'static [&'static str],
    /// `false` for the reserved surfaces' groups and for an intermediate
    /// group that is itself a prefix (`auth token`): the shared group
    /// dispatch resolves only the shared ones, and the catalog document
    /// reports the flag as it stands.
    shared: bool,
}

/// Every group the records describe, in registry first-occurrence order.
static GROUPS: LazyLock<Vec<Group>> = LazyLock::new(derive_groups);

/// Derive the group table from the registry records.
///
/// A record name past its first token is a group prefix plus a subcommand
/// label. When every label under one prefix shares a first word, that word is
/// an intermediate group (`auth token`): the prefix lists it alone, and the
/// intermediate's own verbs hang off it.
fn derive_groups() -> Vec<Group> {
    let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for spec in crate::cli_command_registry::all() {
        let Some((prefix, rest)) = spec.name.split_once(' ') else {
            continue;
        };
        let index = if let Some(index) = groups.iter().position(|(name, _)| *name == prefix) {
            index
        } else {
            groups.push((prefix, Vec::new()));
            groups.len() - 1
        };
        if !groups[index].1.contains(&rest) {
            groups[index].1.push(rest);
        }
    }
    let mut derived: Vec<Group> = Vec::new();
    for (prefix, subs) in groups {
        let first_word = subs
            .first()
            .and_then(|sub| sub.split_once(' '))
            .map(|(word, _)| word);
        let collapses = first_word.is_some()
            && subs
                .iter()
                .all(|sub| sub.split_once(' ').map(|(word, _)| word) == first_word);
        if !collapses {
            derived.push(Group {
                prefix,
                subs: leak_subs(subs),
                shared: is_shared(prefix),
            });
            continue;
        }
        // Every label is `<word> <verb>`: `word` is the group the prefix
        // lists, and its own verbs are the labels past that word.
        let word = first_word.unwrap_or_default();
        let mut inner: Vec<&'static str> = Vec::new();
        for sub in &subs {
            if let Some((_, rest)) = sub.split_once(' ')
                && !inner.contains(&rest)
            {
                inner.push(rest);
            }
        }
        let inner_prefix: &'static str = Box::leak(format!("{prefix} {word}").into_boxed_str());
        derived.push(Group {
            prefix,
            subs: leak_subs(vec![word]),
            shared: is_shared(prefix),
        });
        derived.push(Group {
            prefix: inner_prefix,
            subs: leak_subs(inner),
            shared: false,
        });
    }
    derived
}

/// Leak `subs` once, because the lookups return `&'static`.
fn leak_subs(subs: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(subs.into_boxed_slice())
}

/// Whether `prefix` is a shared group rather than a reserved surface's.
fn is_shared(prefix: &str) -> bool {
    !SURFACE_OWNED_GROUPS.contains(&prefix)
}

/// Every derived group, in derived order: the prefix, its subcommand labels,
/// and whether the shared group machinery resolves it. The catalog document
/// serializes this table, so a reader sees the groups the records derive.
pub(crate) fn all_groups() -> impl Iterator<Item = (&'static str, &'static [&'static str], bool)> {
    GROUPS
        .iter()
        .map(|group| (group.prefix, group.subs, group.shared))
}

/// The subcommands of `cmd` when it is a shared group.
pub(crate) fn group_subcommands(cmd: &str) -> Option<&'static [&'static str]> {
    let group = GROUPS.iter().find(|group| group.prefix == cmd)?;
    group.shared.then_some(group.subs)
}

/// The subcommands of `verb`, from the derived group table.
pub(crate) fn subcommands_of(verb: &str) -> Option<&'static [&'static str]> {
    GROUPS
        .iter()
        .find(|group| group.prefix == verb)
        .map(|group| group.subs)
}

/// Every top-level verb the CLI accepts, in the suggestion order the ties
/// resolve by.
///
/// The set is the registry's own top-level verbs — each single-token record
/// names itself, each multi-token record extends its prefix — and the
/// completeness pin in `cli_help_tests` fails if the registry and this order
/// disagree in either direction. The order itself is the authored one,
/// because the hints a reader sees must not move in a mechanical phase. The
/// tie it decides is pinned by
/// `cli_help_tests::suggest_command_ties_resolve_by_the_authored_order`: `s`
/// scores `sync` and `show` equally and keeps `sync` here, where the
/// registry's own order keeps `show` instead.
static KNOWN_VERB_ORDER: &[&str] = &[
    "version",
    "health",
    "help",
    "mcp",
    "ops",
    "retention",
    "compliance",
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

/// Every top-level verb the CLI accepts, in suggestion order.
pub(crate) fn known_commands() -> Vec<&'static str> {
    KNOWN_VERB_ORDER.to_vec()
}

/// Every command the CLI will run, as the full name a reader would type.
///
/// A prefix that names further subcommands (`auth token`) is replaced by the
/// commands under it, because that is how the catalog reads. This is the one
/// list a completeness check can compare the catalog against.
#[cfg(test)]
pub(crate) fn all_known_commands() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = Vec::new();
    for verb in known_commands() {
        match subcommands_of(verb) {
            Some(subs) => all.extend(subs.iter().map(|sub| leak_name(verb, sub))),
            None => all.push(verb),
        }
    }
    // A prefix that names further subcommands is a prefix rather than a
    // command; its own verbs take its place.
    let mut index = 0;
    while index < all.len() {
        match subcommands_of(all[index]) {
            Some(subs) => {
                let name = all.swap_remove(index);
                for sub in subs.iter().rev() {
                    all.insert(index, leak_name(name, sub));
                }
            }
            None => index += 1,
        }
    }
    all
}

/// The name of a multi-token command, kept for the life of the process.
///
/// The command tables are static, so this runs once per process at most, and a
/// `&'static str` is what the catalog and its tests both want to compare against.
#[cfg(test)]
fn leak_name(verb: &str, sub: &str) -> &'static str {
    Box::leak(format!("{verb} {sub}").into_boxed_str())
}
