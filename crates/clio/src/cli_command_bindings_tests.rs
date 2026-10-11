// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the global order accessor: each spine is the order its
//! surface prints, every spine key resolves exactly one row from exactly one
//! source, and no authored fallback row is left for a command the registry
//! holds beyond the eight byte-reference rows the record tests pin.
//!
//! ## Owns
//! - The one-row-per-key invariant: a registered key prints its record, an
//!   unregistered key its one authored row, and every record that binds a tool
//!   and every registered command sits on its spine
//! - Source separation: a registered command's row comes from its record, and
//!   an authored row that remains belongs to a command with no record
//! - Literal rows for one record-first key and one fallback key per surface
//!
//! ## Does not own
//! - The record model (`cli_command_spec_tests`) or a record's own bytes
//!   (`cli_command_registry_*_tests`)
//! - The rendered bytes of either surface (`command_catalog_tests`,
//!   `cli_help_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the accessor's order and resolution.

use super::{BINDING_ORDER, CATALOG_ORDER, STATIC_BINDINGS, catalog_rows, command_bindings};
use crate::cli_command_registry as registry;

/// Every authored catalog row, whatever block it sits in.
fn authored_catalog_rows() -> impl Iterator<Item = (&'static str, &'static str)> {
    crate::command_catalog::ENTRIES_BEFORE
        .iter()
        .chain(crate::command_catalog::ENTRIES_AFTER.iter())
        .copied()
}

/// One surface's row for a command, when the surface lists it.
fn row_of(
    rows: &[(&'static str, &'static str)],
    name: &str,
) -> Option<(&'static str, &'static str)> {
    rows.iter().copied().find(|(command, _)| *command == name)
}

/// Every binding key resolves to exactly one row: the record's tool when the
/// registry holds the command, else its one static row.
///
/// A key served by both sources would print a record and a stale row for the
/// same command; a key served by neither would panic at render time. The count
/// proves every record that binds a tool is on this spine at all.
#[test]
fn every_binding_key_resolves_record_first_or_once_from_the_table() {
    let mut from_records = 0usize;
    for (key, row) in BINDING_ORDER.iter().zip(command_bindings()) {
        let authored: Vec<(&'static str, &'static str)> = STATIC_BINDINGS
            .iter()
            .copied()
            .filter(|(command, _)| command == key)
            .collect();
        if let Some(spec) = registry::find(key) {
            let tool = spec
                .tool
                .unwrap_or_else(|| panic!("`{key}` is on the binding spine but binds no tool"));
            assert_eq!(row, (spec.name, tool), "`{key}` must print its record");
            assert!(
                authored.is_empty(),
                "`{key}` has a record and the static row {authored:?}, so its row has two sources"
            );
            from_records += 1;
        } else {
            assert_eq!(
                authored.len(),
                1,
                "`{key}` has no record and {authored:?} in the static table"
            );
            assert_eq!(row, authored[0], "`{key}` must print its static row");
        }
    }
    assert_eq!(
        from_records,
        registry::all().filter(|spec| spec.tool.is_some()).count(),
        "every record that binds a tool must be on the binding spine"
    );
}

/// Every catalog key resolves to exactly one row: the record's summary when the
/// registry holds the command, else its one authored row.
///
/// The catalog's counterpart to the binding check: the authored tables and the
/// records are two sources for one row per command, never both and never none.
#[test]
fn every_catalog_key_resolves_record_first_or_once_from_the_table() {
    let mut from_records = 0usize;
    for (key, row) in CATALOG_ORDER.iter().zip(catalog_rows()) {
        let authored: Vec<(&'static str, &'static str)> = authored_catalog_rows()
            .filter(|(command, _)| command == key)
            .collect();
        if let Some(spec) = registry::find(key) {
            assert_eq!(
                row,
                (spec.name, spec.summary),
                "`{key}` must print its record"
            );
            // The eight reference rows are the byte reference the reserved-
            // surface record tests pin against; they never render, because a
            // registered key resolves to its record first.
            assert!(
                authored.is_empty()
                    || crate::command_catalog::AUTHORED_ROW_REFERENCES.contains(key),
                "`{key}` has a record and the authored row {authored:?}, so its row has two sources"
            );
            from_records += 1;
        } else {
            assert_eq!(
                authored.len(),
                1,
                "`{key}` has no record and {authored:?} in the authored tables"
            );
            assert_eq!(row, authored[0], "`{key}` must print its authored row");
        }
    }
    assert_eq!(
        from_records,
        registry::all().count(),
        "every registered command must be on the catalog spine"
    );
}

/// One record-first key and one fallback key render their literal rows.
///
/// The accessors map over the spine arrays, so spine order and row count hold
/// by construction and a missing key panics inside the accessor; the
/// independent inputs here are the registry record and the static table, which
/// must stay the two sources a key resolves from.
#[test]
fn a_record_key_and_a_fallback_key_render_their_literal_rows() {
    let bindings = command_bindings();
    let rows = catalog_rows();
    assert_eq!(
        row_of(&bindings, "persona get"),
        Some(("persona get", "persona_get")),
        "a registered key prints its record's binding row"
    );
    assert_eq!(
        row_of(&rows, "persona get"),
        Some(("persona get", "the persona model, within a token budget")),
        "a registered key prints its record's catalog row"
    );
    assert_eq!(
        row_of(&bindings, "sync pull"),
        Some(("sync pull", "sync_pull")),
        "an unregistered key prints its static row"
    );
}

/// No spine names the same key twice, so one command cannot print twice.
#[test]
fn each_spine_names_every_key_once() {
    for (label, spine) in [("binding", BINDING_ORDER), ("catalog", CATALOG_ORDER)] {
        let mut keys: Vec<&str> = spine.to_vec();
        keys.sort_unstable();
        for pair in keys.windows(2) {
            assert_ne!(pair[0], pair[1], "{label} spine repeats `{}`", pair[0]);
        }
    }
}

/// Every registered command is already a spine key, so a record added to the
/// registry slots into its original position without an order edit.
#[test]
fn every_registered_command_is_a_spine_key() {
    for spec in registry::all() {
        assert!(
            CATALOG_ORDER.contains(&spec.name),
            "`{}` is registered but missing from the catalog spine",
            spec.name
        );
        if spec.tool.is_some() {
            assert!(
                BINDING_ORDER.contains(&spec.name),
                "`{}` binds a tool but is missing from the binding spine",
                spec.name
            );
        }
    }
}

/// Every binding key is a catalog row, so the two surfaces cannot advertise
/// a command the catalog does not list.
#[test]
fn every_binding_key_is_a_catalog_key() {
    for key in BINDING_ORDER {
        assert!(
            CATALOG_ORDER.contains(key),
            "`{key}` is bound but is not a catalog row"
        );
    }
}

/// Every authored row still in a table belongs to a command the registry does
/// not hold, so no command's row has both a record and a stale authored copy.
///
/// The eight subcommand reference rows are the one exception: they are the byte
/// reference the reserved-surface record tests pin equal to their records'
/// summaries, and they never render, because a registered key resolves to its
/// record first.
///
/// This is the tripwire that keeps the deletion honest in both directions: a
/// record dropped from the registry would leave a row with no record and no
/// authored row (caught above), and a row left behind after a move fails here.
#[test]
fn every_authored_fallback_row_belongs_to_an_unregistered_command() {
    for (command, _) in STATIC_BINDINGS
        .iter()
        .copied()
        .chain(authored_catalog_rows())
    {
        assert!(
            registry::find(command).is_none()
                || crate::command_catalog::AUTHORED_ROW_REFERENCES.contains(&command),
            "`{command}` is authored in a fallback table and also registered"
        );
    }
}

/// Every authored fallback row is reachable: its command names a spine key, and
/// the surface prints that row for it.
///
/// A row whose command has no spine key is text no command can print, the same
/// defect a duplicate table entry is; the value check keeps the fallback path
/// from rotting while only the record-first path is exercised.
#[test]
fn every_authored_fallback_row_is_reachable_from_its_spine() {
    let bindings = command_bindings();
    let rows = catalog_rows();
    for (command, tool) in STATIC_BINDINGS {
        assert!(
            BINDING_ORDER.contains(command),
            "`{command}` has a static binding row but no binding spine key"
        );
        assert_eq!(
            row_of(&bindings, command),
            Some((*command, *tool)),
            "binding row for `{command}`"
        );
    }
    for (command, summary) in authored_catalog_rows() {
        assert!(
            CATALOG_ORDER.contains(&command),
            "`{command}` has an authored catalog row but no catalog spine key"
        );
        assert_eq!(
            row_of(&rows, command),
            Some((command, summary)),
            "catalog row for `{command}`"
        );
    }
}
