// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the global order accessor: each spine is the order its
//! surface prints, every spine key resolves to exactly one registry record,
//! and the spines and the registry list the same commands.
//!
//! ## Owns
//! - The one-record-per-key invariant: every spine key prints its record's
//!   row, and every registered command sits on its spine
//! - The spine keys' uniqueness and the binding spine's coverage of every
//!   record that binds a tool
//! - Literal rows for one binding key and one catalog key
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

use super::{BINDING_ORDER, CATALOG_ORDER, catalog_rows, command_bindings};
use crate::cli_command_registry as registry;

/// One surface's row for a command, when the surface lists it.
fn row_of(
    rows: &[(&'static str, &'static str)],
    name: &str,
) -> Option<(&'static str, &'static str)> {
    rows.iter().copied().find(|(command, _)| *command == name)
}

/// Every binding key resolves to its record's tool, and every record that
/// binds a tool is on this spine.
///
/// A key with no record, or a record with no tool, already panics inside the
/// accessor before the loop here runs, so the loop's own arms cannot fire;
/// the named failure this test adds is a row list shorter than the spine,
/// which `zip` alone would prefix-compare in silence. The count proves every
/// tool-binding record is on the spine at all.
#[test]
fn every_binding_key_resolves_to_its_record() {
    let bindings = command_bindings();
    assert_eq!(
        bindings.len(),
        BINDING_ORDER.len(),
        "the binding accessor must print one row per spine key"
    );
    for (key, row) in BINDING_ORDER.iter().zip(bindings) {
        let spec = registry::find(key)
            .unwrap_or_else(|| panic!("`{key}` is on the binding spine with no record"));
        let tool = spec
            .tool
            .unwrap_or_else(|| panic!("`{key}` is on the binding spine but binds no tool"));
        assert_eq!(row, (spec.name, tool), "`{key}` must print its record");
    }
    let spine_tools: usize = BINDING_ORDER
        .iter()
        .filter(|key| registry::find(key).is_some_and(|spec| spec.tool.is_some()))
        .count();
    assert_eq!(
        spine_tools,
        registry::all().filter(|spec| spec.tool.is_some()).count(),
        "every record that binds a tool must be on the binding spine"
    );
}

/// Every catalog key resolves to its record's summary, and every registered
/// command is a catalog key.
///
/// The catalog's counterpart to the binding check: the spine and the registry
/// are one source for one row per command, so a command on one and missing
/// from the other fails here. The length assertion names a row list shorter
/// than the spine, which `zip` alone would prefix-compare in silence.
#[test]
fn every_catalog_key_resolves_to_its_record() {
    let rows = catalog_rows();
    assert_eq!(
        rows.len(),
        CATALOG_ORDER.len(),
        "the catalog accessor must print one row per spine key"
    );
    for (key, row) in CATALOG_ORDER.iter().zip(rows) {
        let spec = registry::find(key)
            .unwrap_or_else(|| panic!("`{key}` is on the catalog spine with no record"));
        assert_eq!(
            row,
            (spec.name, spec.summary),
            "`{key}` must print its record"
        );
    }
    assert_eq!(
        registry::all().count(),
        CATALOG_ORDER.len(),
        "every registered command must be on the catalog spine"
    );
}

/// One binding key and one catalog key render their literal rows.
///
/// The accessors map over the spine arrays, so spine order and row count hold
/// by construction and a missing key panics inside the accessor; the
/// independent inputs here are two records' bytes.
#[test]
fn a_binding_key_and_a_catalog_key_render_their_literal_rows() {
    let bindings = command_bindings();
    let rows = catalog_rows();
    assert_eq!(
        row_of(&bindings, "persona get"),
        Some(("persona get", "persona_get")),
        "the key prints its record's binding row"
    );
    assert_eq!(
        row_of(&rows, "persona get"),
        Some(("persona get", "the persona model, within a token budget")),
        "the key prints its record's catalog row"
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
