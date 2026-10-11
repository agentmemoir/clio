// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the registry seam of the `clio help` catalog: each
//! record's row appears once, at its spine position among the remaining
//! authored rows, with no stale authored copy beside it, and every rendered
//! registered block shows only flags its parser accepts.
//!
//! ## Owns
//! - The one-row-per-record invariant and the record spans' spine bounds
//! - The authored reference rows: which stay, and their byte equality with the
//!   record summaries they reference
//! - The rendered registered blocks' flag check, with its bounded prose
//!   exemption printed separately
//!
//! ## Does not own
//! - The catalog's own parity with a command's `--help`
//!   (`command_catalog_tests`)
//! - The record model (`cli_command_spec_tests`) or a record's own bytes
//!   (the `cli_command_registry_*_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the catalog's registry seam.

use super::{AUTHORED_ROW_REFERENCES, ENTRIES_AFTER, ENTRIES_BEFORE, catalog_rows};
use crate::cli_help_usage;
use crate::help_flags_test_support::{entry_block, flags_in};

/// The catalog text, which is what the tests below read instead of the entry
/// table: the claim under test is about what a reader sees.
fn catalog_text() -> String {
    super::catalog_text()
}

/// The catalog's block for one command: its usage lines and its description.
fn catalog_block(command: &str) -> String {
    entry_block(&catalog_text(), command)
}

/// Every command in the registry is listed by `catalog_commands` exactly once,
/// so no record's row can be duplicated or dropped.
#[test]
fn every_registry_command_is_listed_exactly_once() {
    let listed: Vec<&str> = super::catalog_commands().collect();
    for spec in crate::cli_command_registry::all() {
        let count = listed.iter().filter(|name| **name == spec.name).count();
        assert_eq!(
            count, 1,
            "`{}` must be listed once, found {count} rows",
            spec.name
        );
    }
}

/// The registry's records render at their spine positions: the reserved heads
/// lead the catalog as one span, the group records run unbroken from `recall`
/// to `graph link`, and `install`, `uninstall`, and `help` close it; the
/// authored rows that remain sit between the spans, never inside one.
///
/// A catalog that compacted the records elsewhere, dropped one, or let an
/// authored row slip inside a span fails here.
#[test]
fn the_registry_records_render_at_their_spine_positions() {
    let listed: Vec<&str> = super::catalog_commands().collect();
    let is_record = |name: &str| crate::cli_command_registry::find(name).is_some();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (index, name) in listed.iter().enumerate() {
        if !is_record(name) {
            continue;
        }
        match spans.last_mut() {
            Some(span) if span.1 + 1 == index => span.1 = index,
            _ => spans.push((index, index)),
        }
    }
    let bounds: Vec<(&str, &str)> = spans
        .iter()
        .map(|(start, end)| (listed[*start], listed[*end]))
        .collect();
    assert_eq!(
        bounds,
        [
            ("version", "setup"),
            ("recall", "graph link"),
            ("install", "help")
        ],
        "the record spans' bounds changed; move them only when the catalog's \
         record shape is intended to change"
    );
    let rendered: usize = spans.iter().map(|(start, end)| end - start + 1).sum();
    assert_eq!(
        rendered,
        crate::cli_command_registry::all().count(),
        "every record prints exactly once across the spans"
    );
}

/// No authored row names a registered command, except the eight subcommand
/// rows the reserved-surface record tests read as their byte reference: those
/// stay, and this check pins each one byte-equal to its record's summary, so a
/// registered command's row still has exactly one rendered source.
#[test]
fn no_authored_row_names_a_registered_command() {
    for (command, summary) in ENTRIES_BEFORE.iter().chain(ENTRIES_AFTER.iter()) {
        let Some(spec) = crate::cli_command_registry::find(command) else {
            continue;
        };
        assert!(
            AUTHORED_ROW_REFERENCES.contains(command),
            "`{command}` is authored in the catalog table and also registered"
        );
        assert_eq!(
            *summary, spec.summary,
            "`{command}`'s authored row and its record's summary"
        );
    }
}

/// Every registered command resolves a usage line and a summary through the
/// catalog, so its row renders rather than falling back to a marker.
#[test]
fn every_registry_command_resolves_a_usage_line_and_summary() {
    for spec in crate::cli_command_registry::all() {
        assert!(
            cli_help_usage::usage_line(spec.name).is_some(),
            "`{}` has no usage line",
            spec.name
        );
        let row = catalog_rows()
            .find(|(name, _)| *name == spec.name)
            .expect("a registered name is a catalog row");
        assert_eq!(row.1, spec.summary, "`{}` summary", spec.name);
        // `remember` renders no description: its authored catalog row was
        // empty, and the frozen golden capture pins those bytes, so the record
        // carries an empty summary too. Every other summary must stay filled.
        if spec.name != "remember" {
            assert!(!row.1.is_empty(), "`{}` summary is empty", spec.name);
        }
    }
}

/// A registered command whose `clio help` block names other commands' flags by
/// design: the help record's note is the catalog's own global-flags and
/// exit-codes prose, so a per-command parser check cannot apply to it, and its
/// bytes stay pinned by the catalog row tests and the frozen capture instead.
/// Bounded: a command added here needs the same stated reason.
const PROSE_EXEMPT: &[&str] = &["help"];

/// The rendered `clio help` block for a registered command shows only flags its
/// parser accepts.
///
/// The usage-line parity check compares the registry text; this one reads the
/// rendered block a reader actually sees, so a renderer, wrapping, or note
/// change that puts a flag into the catalog that the command's parser would
/// refuse fails here even though the usage line itself never changed.
#[test]
fn every_registered_catalog_block_shows_only_flags_its_parser_accepts() {
    for spec in crate::cli_command_registry::all() {
        if PROSE_EXEMPT.contains(&spec.name) {
            continue;
        }
        let accepted = accepted_block_flags(spec.name).unwrap_or_else(|| {
            panic!(
                "`{}` has no parser spec; the loop sources names from the registry, \
                 so a missing spec is a new undecidable shape, not a silent pass",
                spec.name
            )
        });
        let block = catalog_block(spec.name);
        let wrong: Vec<String> = flags_in(&block)
            .into_iter()
            .filter(|flag| !accepted.contains(flag))
            .collect();
        assert!(
            wrong.is_empty(),
            "`clio {}` shows {wrong:?} in its `clio help` block, which its parser \
             refuses:\n{block}",
            spec.name
        );
    }
    println!("catalog-block prose exemptions ({}):", PROSE_EXEMPT.len());
    for name in PROSE_EXEMPT {
        println!("  clio {name}");
    }
}

/// The `--flag` names the command's real parser accepts, or `None` when the
/// name reaches no record and no group parser (the test above only passes
/// registered names, so `None` there is a new shape, never an expected path).
///
/// A read/write group verb parses through its group's `FlagSpec`. A reserved
/// surface parses through the shared `parse_flags` gate over its record-derived
/// names plus every declared global, so its accepted set is the record's
/// documented names (with the hidden ones the derivation adds) and the globals.
/// `setup` is the one exception: its wizard parser reads the record's
/// documented entries alone and refuses the globals the other surfaces accept.
fn accepted_block_flags(command: &str) -> Option<Vec<String>> {
    if let Some(spec) =
        crate::cli_read::spec_for(command).or_else(|| crate::cli_write::spec_for(command))
    {
        let mut names: Vec<String> = spec
            .value_flags()
            .iter()
            .chain(spec.bool_flags().iter())
            .map(|name| format!("--{name}"))
            .collect();
        names.sort_unstable();
        return Some(names);
    }
    let record = crate::cli_command_registry::find(command)?;
    let mut names: Vec<String> = crate::cli_command_registry_surface::parse_flag_names(command)
        .into_iter()
        .map(|name| format!("--{name}"))
        .collect();
    if record.name != "setup" {
        names.extend(
            crate::cli_command_registry::global_value_names()
                .chain(crate::cli_command_registry::global_bool_names())
                .map(|name| format!("--{name}")),
        );
    }
    names.sort_unstable();
    names.dedup();
    Some(names)
}

/// The override record's rendered block hides the flag its parser accepts: the
/// printed line comes from the record's `usage_override`, not from its
/// documented flag set, so a renderer that ignored the override would print
/// `--context` here.
#[test]
fn the_persona_observe_block_hides_the_context_flag_its_parser_accepts() {
    let block = catalog_block("persona observe");
    assert!(
        !block.contains("--context"),
        "the override must keep --context out of the catalog: {block}"
    );
}
