// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the scope-ledger tests: the named family set,
//! the four subject-link fixture classes, the unclassified and absent-source
//! rules, the link-removal rerun, and the caller-copy custody rule.
//!
//! ## Owns
//! - Every assertion about `scope_inventory` output
//!
//! ## Does not own
//! - The ledger itself and the fixture store (`inventory_fixture.rs`)
//! - The subject-link-class tests declared here (`inventory_link_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{
    Custody, InventoryBackend, InventoryFacts, InventoryInput, InventoryPaths, InventoryRow, Owner,
    RetentionAction, ScopeInventory, scope_inventory,
};
use crate::inventory_fixture::LedgerFixture;
use crate::test_unique::unique;
use clio_store::{PostgresTargetState, SqliteFileState, sqlite_file_state};
use std::path::Path;

/// Every row family the ledger reports, with no caller copy declared.
const EXPECTED_FAMILIES: &[&str] = &[
    "assoc_edges",
    "backup_archives",
    "bank_structures",
    "beliefs",
    "derived_item_structures",
    "enabled_peers",
    "erasure_journal",
    "events",
    "export_copies",
    "failures",
    "items",
    "kms_file",
    "legacy_failures",
    "legacy_tasks",
    "legacy_tombstones",
    "persona_erase_tombstones",
    "persona_preferences",
    "persona_stable",
    "restore_routes",
    "stderr_events",
    "store_file",
    "subject_keys",
    "sync_state",
    "tasks",
    "tombstones",
    "triples",
];

/// The one row a report holds for `family`.
fn row_for<'a>(ledger: &'a ScopeInventory, family: &str) -> &'a InventoryRow {
    ledger
        .rows
        .iter()
        .find(|row| row.family == family)
        .unwrap_or_else(|| panic!("the ledger has no {family} row"))
}

/// The named open reasons of a family's row (empty when the row is closed).
fn reason<'a>(ledger: &'a ScopeInventory, family: &str) -> &'a str {
    row_for(ledger, family)
        .open_reason
        .as_deref()
        .unwrap_or_default()
}

/// Paths that no fixture ever writes, under one unused directory name.
fn absent_paths() -> InventoryPaths {
    let root = std::env::temp_dir().join(unique("inv-fx-absent"));
    InventoryPaths {
        db_path: root.join("clio.db"),
        kms_path: root.join("clio.db.kms.json"),
        backup_dir: root.join("backups"),
    }
}

/// A ledger input over hand-built facts and paths.
fn probe_input(facts: InventoryFacts, paths: InventoryPaths) -> InventoryInput {
    InventoryInput {
        facts,
        paths,
        declared_copies: Vec::new(),
        peers_enabled: None,
    }
}

#[test]
fn scope_ledger_names_every_family_with_a_source_and_a_link_or_a_gap() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), None));

    let mut families: Vec<&str> = ledger.rows.iter().map(|row| row.family).collect();
    families.sort_unstable();
    assert_eq!(
        families, EXPECTED_FAMILIES,
        "the named family set is pinned"
    );
    assert_eq!(ledger.backend, InventoryBackend::Sqlite);
    assert_eq!(serde_json::to_value(ledger.backend).unwrap(), "sqlite");
    assert!(
        ledger.unclassified.is_empty(),
        "every live table is named by a row: {:?}",
        ledger.unclassified
    );
    assert!(
        !ledger.complete,
        "a scope with named gaps is never complete"
    );

    for row in &ledger.rows {
        assert!(!row.source.is_empty(), "row {} has no source", row.family);
        assert!(
            row.subject_link.is_some()
                || row.gap.is_some()
                || matches!(row.family, "store_file" | "kms_file" | "backup_archives"),
            "row {} has a link, a gap, or a resolved local path",
            row.family
        );
        assert!(!row.proof.is_empty(), "row {} has no proof", row.family);
        assert_eq!(
            row.open,
            row.open_reason.is_some(),
            "row {} disagrees about being open",
            row.family
        );
        assert!(
            row.gap.is_none() || row.open,
            "row {} has a named gap, so it must be open",
            row.family
        );
    }

    // The three records that exist nowhere stay open and say what is missing.
    for family in ["erasure_journal", "stderr_events", "enabled_peers"] {
        let row = row_for(&ledger, family);
        assert!(row.open, "{family} must stay open");
        assert_eq!(row.retention_action, RetentionAction::Open, "{family}");
    }
    assert!(
        reason(&ledger, "enabled_peers").contains("could not be verified"),
        "{:?}",
        row_for(&ledger, "enabled_peers").open_reason
    );

    // The frozen JSON field names and enum spellings.
    let items = serde_json::to_value(row_for(&ledger, "items")).unwrap();
    let mut keys: Vec<&str> = items
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys.join(","),
        "custody,family,gap,key_path,open,open_reason,owner,proof,readability,retention_action,source,subject_link"
    );
    assert_eq!(items["owner"], "clio");
    assert_eq!(items["retention_action"], "erase");
    assert_eq!(items["subject_link"], "items.subject_id");
}

#[test]
fn live_tables_named_by_no_row_stay_unclassified() {
    let facts = SqliteFileState {
        path: absent_paths().db_path,
        exists: true,
        bytes: 4_096,
        table_counts: vec![("items".to_owned(), 1), ("unexpected_family".to_owned(), 2)],
        banks: vec!["inv-fx-bank".to_owned()],
    };
    let ledger = scope_inventory(&probe_input(InventoryFacts::Sqlite(facts), absent_paths()));

    assert_eq!(ledger.unclassified, ["unexpected_family"]);
    assert!(!ledger.complete, "an unclassified table is not complete");
    // A row whose tables are not in the facts is open, and names them.
    assert!(row_for(&ledger, "tasks").open && reason(&ledger, "tasks").contains("task_records"));
    assert!(row_for(&ledger, "events").open && reason(&ledger, "events").contains("audit_events"));
}

#[test]
fn absent_sources_and_paths_stay_open_with_named_reasons() {
    let facts = SqliteFileState {
        path: absent_paths().db_path,
        exists: false,
        bytes: 0,
        table_counts: Vec::new(),
        banks: Vec::new(),
    };
    let ledger = scope_inventory(&probe_input(InventoryFacts::Sqlite(facts), absent_paths()));

    assert_eq!(ledger.rows.len(), EXPECTED_FAMILIES.len());
    assert!(
        ledger.rows.iter().all(|row| row.open),
        "an absent store is never a green inventory"
    );
    assert!(!ledger.complete);
    assert!(ledger.unclassified.is_empty(), "{:?}", ledger.unclassified);
    for (family, expected) in [
        ("store_file", "store file not found"),
        ("kms_file", "key file not found"),
        ("backup_archives", "backup archives root not found"),
        ("erasure_journal", "no erasure journal exists"),
        ("stderr_events", "no durable stderr event store exists"),
    ] {
        assert!(
            row_for(&ledger, family).open && reason(&ledger, family).contains(expected),
            "{family} must name what is missing: {}",
            reason(&ledger, family)
        );
    }
}

#[test]
fn restore_routes_stay_open_as_a_named_in_scope_area() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), None));

    let row = row_for(&ledger, "restore_routes");
    assert!(row.open, "restore routes are in scope and stay open");
    assert_eq!(row.retention_action, RetentionAction::Open);
    assert!(
        reason(&ledger, "restore_routes")
            .contains("restore routes are code paths, not stored objects"),
        "{}",
        reason(&ledger, "restore_routes")
    );
    assert!(
        reason(&ledger, "restore_routes").contains("cannot reach them"),
        "{}",
        reason(&ledger, "restore_routes")
    );
}

#[test]
fn every_required_source_area_is_named_by_a_planned_row() {
    let uncovered = super::inventory_rows::uncovered_required_areas();
    assert!(
        uncovered.is_empty(),
        "every in-scope source area must stay covered by a planned row: {uncovered:?}"
    );
}

#[test]
fn the_store_file_source_traces_the_wal_and_shm_sidecars() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), None));
    // The store opens in SQLite's default rollback-journal mode, so no
    // -wal/-shm sidecar survives beside the measured file.
    let source = &row_for(&ledger, "store_file").source;
    assert!(
        source.contains("-wal absent") && source.contains("-shm absent"),
        "the sidecar trace names both sidecars: {source}"
    );

    // Sidecar files beside the measured path are traced as present.
    let sidecar_dir = fixture.dir().join("sidecars");
    std::fs::create_dir_all(&sidecar_dir).expect("create the sidecar probe directory");
    let probe_db = sidecar_dir.join("clio.db");
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = probe_db.as_os_str().to_owned();
        sidecar.push(suffix);
        std::fs::write(Path::new(&sidecar), b"").expect("write the probe sidecar");
    }
    let facts = SqliteFileState {
        path: probe_db.clone(),
        exists: true,
        bytes: 4_096,
        table_counts: vec![("items".to_owned(), 1)],
        banks: Vec::new(),
    };
    let probe = scope_inventory(&probe_input(
        InventoryFacts::Sqlite(facts),
        InventoryPaths {
            db_path: probe_db,
            kms_path: fixture.kms_path.clone(),
            backup_dir: fixture.backup_dir,
        },
    ));
    let probe_source = &row_for(&probe, "store_file").source;
    assert!(
        probe_source.contains("-wal present") && probe_source.contains("-shm present"),
        "existing sidecar files are traced as present: {probe_source}"
    );
}

#[test]
fn enabled_peers_with_the_enablement_fact_stay_open_without_acknowledgements() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), Some(true)));

    let row = row_for(&ledger, "enabled_peers");
    assert!(row.open, "enabled peers without acknowledgements stay open");
    assert_eq!(row.retention_action, RetentionAction::Open);
    assert!(
        reason(&ledger, "enabled_peers")
            .contains("peers are enabled and no acknowledgement record exists to verify them"),
        "{}",
        reason(&ledger, "enabled_peers")
    );
}

#[test]
fn a_postgres_target_names_the_same_families_as_sqlite() {
    let fixture = LedgerFixture::create();
    let sqlite = scope_inventory(&fixture.input(Vec::new(), None));
    let measured = sqlite_file_state(&fixture.db_path).expect("measure the fixture store");
    let target = PostgresTargetState {
        schema: "inv_fx".to_owned(),
        tables: Vec::new(),
        table_counts: measured.table_counts,
        banks: measured.banks,
    };
    let ledger = scope_inventory(&probe_input(
        InventoryFacts::Postgres(target),
        fixture.paths(),
    ));

    assert_eq!(ledger.backend, InventoryBackend::Postgres);
    let mut families: Vec<&str> = ledger.rows.iter().map(|row| row.family).collect();
    families.sort_unstable();
    let mut sqlite_families: Vec<&str> = sqlite.rows.iter().map(|row| row.family).collect();
    sqlite_families.sort_unstable();
    assert_eq!(families, sqlite_families, "the same named family set");
    assert!(
        row_for(&ledger, "store_file")
            .source
            .contains("postgres target schema")
    );
    assert!(!ledger.complete);
}

#[test]
fn a_declared_caller_copy_keeps_custody_and_is_never_deleted() {
    let fixture = LedgerFixture::create();
    let copy = fixture.caller_path("caller-export.json");
    write_file(&copy, "caller-held export bytes");
    let unrelated = fixture.dir().join("unrelated.txt");
    write_file(&unrelated, "unrelated bytes");

    let ledger = scope_inventory(&fixture.input(vec![copy.clone()], Some(false)));

    let copy_row = row_for(&ledger, "export_copies");
    assert_eq!(copy_row.custody, Custody::Caller);
    assert_eq!(copy_row.owner, Owner::Operator);
    assert_eq!(copy_row.retention_action, RetentionAction::Record);
    assert!(copy_row.open);
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        "caller-held export bytes",
        "the inventory writes nothing over a declared copy"
    );
    assert!(unrelated.exists(), "no unrelated file is deleted");
    assert!(fixture.db_path.exists(), "the store file survives the run");
    assert!(!ledger.complete);
}

#[test]
fn a_declared_copy_that_is_not_found_stays_open_with_its_path() {
    let fixture = LedgerFixture::create();
    let missing = fixture.caller_path("never-written.json");
    let ledger = scope_inventory(&fixture.input(vec![missing.clone()], Some(false)));

    let copy_row = row_for(&ledger, "export_copies");
    let why = reason(&ledger, "export_copies");
    assert!(
        copy_row.open && why.contains("declared copy not found"),
        "{why}"
    );
    assert!(why.contains(&missing.display().to_string()), "{why}");
    assert!(!ledger.complete);
}

/// Write one fixture file, creating its parent directory.
fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture directory");
    }
    std::fs::write(path, text).expect("write the fixture file");
}

#[path = "inventory_link_tests.rs"]
mod inventory_link_tests;
