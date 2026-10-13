// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres half of the scope-ledger
//! tests: the same named family set as SQLite, the four subject-link classes
//! read back from a real scoped store, the per-backend source handling, and the
//! credential rule.
//!
//! ## Owns
//! - Every assertion about a ledger built from `InventoryFacts::Postgres`,
//!   including the credential rule over row text
//!
//! ## Does not own
//! - The ledger and its row plan (`inventory.rs`, `inventory_rows.rs`)
//! - The Postgres harness and the SQLite fixture, both read-only here
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::inventory::{
    InventoryBackend, InventoryFacts, InventoryInput, InventoryRow, RetentionAction,
    ScopeInventory, scope_inventory,
};
use crate::inventory_fixture::{LedgerFixture, OTHER, SELECTED, SHARED, UNKNOWN, fixture_id};
use crate::inventory_pg_support_tests::{
    PgLedgerFixture, open_pg_ledger_fixture, pg_base_url, pg_skip_note, url_password, url_userinfo,
};
use clio_store::{BeliefStore, ComplianceStore, HistoryStore, PersonaStore, Store};
use clio_types::{AmError, ErrorCode};
#[path = "inventory_pg_triples_tests.rs"]
pub(crate) mod triples;

/// The suite name every skip note in this module carries.
const SUITE: &str = "inventory Postgres ledger tests";

/// The live fixture, or a printed credential-safe skip note plus `None`.
fn pg_fixture_or_skip() -> Option<(clio_store::PgTestGuard, PgLedgerFixture)> {
    match open_pg_ledger_fixture() {
        Ok(pair) => Some(pair),
        Err(err) => {
            eprintln!("{}", pg_skip_note(SUITE, &err));
            None
        }
    }
}

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

/// Every text a tunnel could carry a credential through.
fn row_texts(row: &InventoryRow) -> Vec<&str> {
    [
        Some(row.source.as_str()),
        row.subject_link.as_deref(),
        row.gap.as_deref(),
        row.open_reason.as_deref(),
        row.key_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The ledger for one fixture, measured from the live scoped target at call
/// time.
fn pg_ledger(fixture: &PgLedgerFixture) -> ScopeInventory {
    scope_inventory(&InventoryInput {
        facts: InventoryFacts::Postgres(fixture.target_state()),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: None,
    })
}

/// Every row has a source and a proof, a link or a gap or a local path, and honest open state.
fn assert_rows_are_well_formed(ledger: &ScopeInventory) {
    for row in &ledger.rows {
        assert!(!row.source.is_empty(), "row {} has no source", row.family);
        let local_only = matches!(row.family, "store_file" | "kms_file" | "backup_archives");
        assert!(
            row.subject_link.is_some() || row.gap.is_some() || local_only,
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
}

/// The four records that exist nowhere stay open and say why.
fn assert_absent_records_stay_open(ledger: &ScopeInventory) {
    for family in [
        "erasure_journal",
        "stderr_events",
        "enabled_peers",
        "restore_routes",
    ] {
        let row = row_for(ledger, family);
        assert!(
            row.open && row.retention_action == RetentionAction::Open,
            "{family} must stay open"
        );
        assert!(
            !reason(ledger, family).is_empty(),
            "{family} must name why it is open"
        );
    }
}

#[test]
fn inventory_pg_ledger_names_the_same_families_as_sqlite() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let state = fixture.target_state();
    let ledger = pg_ledger(&fixture);
    let sqlite_fixture = LedgerFixture::create();
    let sqlite = scope_inventory(&sqlite_fixture.input(Vec::new(), None));

    let mut postgres_names: Vec<&str> = ledger.rows.iter().map(|row| row.family).collect();
    postgres_names.sort_unstable();
    let mut sqlite_names: Vec<&str> = sqlite.rows.iter().map(|row| row.family).collect();
    sqlite_names.sort_unstable();
    assert_eq!(
        postgres_names, sqlite_names,
        "the two backends name the same families"
    );
    assert_eq!(postgres_names.len(), 26, "the pinned family set is 26 rows");
    for family in [
        "items",
        "triples",
        "assoc_edges",
        "persona_stable",
        "persona_preferences",
        "tasks",
        "failures",
        "beliefs",
        "tombstones",
    ] {
        assert!(
            postgres_names.contains(&family),
            "the export family {family} is named"
        );
    }

    assert_eq!(ledger.backend, InventoryBackend::Postgres);
    assert_eq!(serde_json::to_value(ledger.backend).unwrap(), "postgres");
    assert!(
        !ledger.complete,
        "a scope with named gaps is never complete"
    );
    let store_file = row_for(&ledger, "store_file");
    assert!(
        store_file.source.contains("postgres target schema"),
        "{}",
        store_file.source
    );
    assert!(
        sqlite.unclassified.is_empty(),
        "every SQLite fixture table is named by a row: {:?}",
        sqlite.unclassified
    );

    assert_rows_are_well_formed(&ledger);
    assert_absent_records_stay_open(&ledger);

    eprintln!(
        "inventory_pg live: schema {} holds {} app tables ({} items rows)",
        state.schema,
        state.table_count(),
        state.count_of("items").unwrap_or(0)
    );
}

#[test]
fn inventory_pg_four_link_classes_exist_on_the_live_store() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let store = &fixture.store;
    let bank = fixture.bank.as_str();

    // selected and other: four records each with a direct `subject_id` link.
    for subject in [SELECTED, OTHER] {
        let id = |role: &str| fixture_id(subject, role);
        let item = store
            .get_memory_item(bank, &id("item"))
            .unwrap()
            .expect("the live item row");
        assert_eq!(item.subject_id, subject);
        let task = store
            .get_task(bank, &id("task"))
            .unwrap()
            .expect("the live task row");
        assert_eq!(task.subject_id, subject);
        let failure = store
            .get_failure(bank, &id("failure-record"))
            .unwrap()
            .expect("the live failure row");
        assert_eq!(failure.subject_id, subject);
        let belief = store
            .get_belief(bank, &id("belief"))
            .unwrap()
            .expect("the live belief row");
        assert_eq!(belief.subject_id, subject);
    }

    // shared: rows that name no subject at all.
    let edges = store.list_assoc_edges(bank, None).unwrap();
    let edge = edges
        .iter()
        .find(|edge| edge.id == fixture_id(SHARED, "edge"))
        .expect("the fixture assoc edge between the two subjects' items");
    assert_eq!(edge.source_id, fixture_id(SELECTED, "item"));
    assert_eq!(edge.target_id, fixture_id(OTHER, "item"));
    let persona = store.get_persona_document(bank).unwrap();
    assert!(
        persona
            .stable
            .iter()
            .any(|entry| entry.key == fixture_id(SHARED, "stable-key"))
    );
    assert!(
        persona
            .preferences
            .iter()
            .any(|pref| pref.key == fixture_id(SHARED, "preference-key"))
    );

    // unknown: the erased subject survives only as a one-way hash.
    let tombstones = store.list_tombstones(&fixture.erased_bank).unwrap();
    let tombstone = tombstones.first().expect("the fixture erasure tombstone");
    assert_eq!(tombstone.request_id, fixture_id(UNKNOWN, "request"));
    assert!(
        !tombstone.subject_hash.is_empty() && tombstone.subject_hash != UNKNOWN,
        "the tombstone link is hashed, never the subject id"
    );
    let erased = store.get_memory_item(&fixture.erased_bank, &fixture_id(UNKNOWN, "item"));
    match erased {
        Ok(None) => {}
        Err(err) => assert_eq!(
            err.code,
            ErrorCode::ErasedSubject,
            "reading an erased subject's item fails closed: {err}"
        ),
        Ok(Some(item)) => panic!("the erased subject's item is still readable: {}", item.id),
    }

    // The ledger's live claims for those classes: a direct link, or a gap.
    let ledger = pg_ledger(&fixture);
    for family in ["items", "tasks", "failures", "beliefs", "subject_keys"] {
        assert!(
            row_for(&ledger, family).subject_link.is_some(),
            "{family} names a direct subject link"
        );
    }
    for family in [
        "assoc_edges",
        "persona_stable",
        "persona_preferences",
        "tombstones",
    ] {
        let row = row_for(&ledger, family);
        assert!(
            row.subject_link.is_none() && row.gap.is_some(),
            "{family} names a gap instead of a link"
        );
    }
}

#[test]
fn inventory_pg_unmeasured_sources_are_named_not_claimed_missing() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let state = fixture.target_state();
    let ledger = pg_ledger(&fixture);

    // The vector table is a live relation the read-only measurement cannot
    // enumerate: it depends on the vector extension's column type.
    let vector_table =
        fixture.catalog_value("SELECT pg_catalog.to_regclass('item_embeddings')::text");
    assert_eq!(
        vector_table.as_deref(),
        Some("item_embeddings"),
        "the scoped store holds the vector table"
    );
    assert!(
        !state
            .tables
            .iter()
            .any(|table| table.name == "item_embeddings")
            && !state
                .table_counts
                .iter()
                .any(|(name, _)| name == "item_embeddings"),
        "the measurement neither lists nor counts the extension-dependent vector table"
    );

    let derived = row_for(&ledger, "derived_item_structures");
    let why = reason(&ledger, "derived_item_structures");
    assert!(derived.open, "the derived row stays open");
    assert!(
        !why.contains("live store facts: item_embeddings"),
        "an extension-dependent relation must not be claimed missing: {why}"
    );
    assert!(
        !why.contains("live store facts: items_fts"),
        "a SQLite-only relation must not be claimed missing: {why}"
    );
    assert!(
        why.contains(
            "item_embeddings* is not enumerated by the read-only target measurement; \
             not individually verified on this backend"
        ),
        "the per-backend handling is named: {why}"
    );
    assert!(
        why.contains("items_fts* is a SQLite-only full-text source"),
        "the SQLite-only source is named per backend: {why}"
    );

    // The unclassified set is reported as measured: the fixture schema names every app table with a row.
    assert!(
        ledger.unclassified.is_empty(),
        "every live Postgres table is named by a row: {:?}",
        ledger.unclassified
    );
    let mut sorted = ledger.unclassified.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted, ledger.unclassified,
        "unclassified is in table-name order without repeats"
    );
    assert!(
        !ledger.complete,
        "the open rows keep complete false whatever the unclassified set is"
    );
}

#[test]
fn inventory_pg_rows_carry_no_database_credential() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let ledger = pg_ledger(&fixture);
    let base = pg_base_url();
    let userinfo = url_userinfo(&base);
    let password = url_password(&base);

    for row in &ledger.rows {
        for text in row_texts(row) {
            assert!(
                !text.contains("://"),
                "row {} printed a connection string",
                row.family
            );
            assert!(
                !text.contains('@'),
                "row {} printed connection userinfo",
                row.family
            );
            if let Some(userinfo) = &userinfo {
                assert!(
                    !text.contains(userinfo.as_str()),
                    "row {} printed the connection userinfo",
                    row.family
                );
            }
            if let Some(password) = &password {
                assert!(
                    !text.contains(password.as_str()),
                    "row {} printed the database password",
                    row.family
                );
            }
        }
    }
    for name in &ledger.unclassified {
        assert!(
            !name.contains("://") && !name.contains('@'),
            "an unclassified table name printed a connection string"
        );
    }
    eprintln!(
        "inventory_pg credential check: {} rows checked, URL carries a password: {}",
        ledger.rows.len(),
        password.is_some()
    );
}

/// A skip note must never carry the connection string or its userinfo.
#[test]
fn inventory_pg_skip_note_carries_no_connection_string() {
    let err = AmError::new(
        ErrorCode::Internal,
        "postgres connect: FATAL: password authentication failed for user \"clio\"",
    );
    let note = pg_skip_note(SUITE, &err);
    assert!(
        !note.contains("://"),
        "a skip note must carry no URL: {note}"
    );
    assert!(
        !note.contains('@'),
        "a skip note must carry no userinfo: {note}"
    );
}

/// Dropping the URL is only worth it if the line still says why the test
/// skipped, so the suite and the server's own words have to survive.
#[test]
fn inventory_pg_skip_note_names_the_suite_and_the_servers_reason() {
    let err = AmError::new(
        ErrorCode::Internal,
        "postgres connect: FATAL: password authentication failed for user \"clio\"",
    );
    let note = pg_skip_note(SUITE, &err);
    assert!(note.contains(SUITE), "{note}");
    assert!(note.contains("password authentication failed"), "{note}");
}
