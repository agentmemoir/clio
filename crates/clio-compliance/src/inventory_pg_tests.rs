// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres half of the scope-ledger
//! tests: the same named family set as SQLite, the four subject-link classes
//! read back from a real scoped store, the per-backend source handling, the
//! credential rule, and the peer fact.
//!
//! ## Owns
//! - Every assertion about a ledger built from `InventoryFacts::Postgres`,
//!   including the credential rule over row text and the peer fact rendered
//!   over both backends' facts
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
    PgLedgerFixture, open_pg_ledger_fixture, pg_base_url, url_password, url_userinfo,
};
use crate::subject_peer::{begin_denial, mark_acknowledged};
use crate::test_unique::unique;
use clio_store::{
    BeliefStore, ComplianceStore, HistoryStore, PeerActionStore, PersonaStore, Store,
};
use clio_types::ErrorCode;
#[path = "inventory_pg_triples_tests.rs"]
pub(crate) mod triples;

/// The suite name every skip note in this module carries.
const SUITE: &str = "inventory Postgres ledger tests";

/// The live fixture, or a printed credential-safe skip note plus `None` — but
/// only under the explicit opt-out: without it an unreachable Postgres fails
/// the suite instead of reporting green.
fn pg_fixture_or_skip() -> Option<(clio_store::PgTestGuard, PgLedgerFixture)> {
    match open_pg_ledger_fixture() {
        Ok(pair) => Some(pair),
        Err(err) => {
            clio_store::pg_skip::unreachable_pg_or_skip(SUITE, &err);
            None
        }
    }
}

/// The one row a report holds for `family`.
fn row_for<'a>(ledger: &'a ScopeInventory, family: &str) -> &'a InventoryRow {
    let row = ledger.rows.iter().find(|row| row.family == family);
    row.unwrap_or_else(|| panic!("the ledger has no {family} row"))
}

/// The named open reasons of a family's row (empty when the row is closed).
fn reason<'a>(ledger: &'a ScopeInventory, family: &str) -> &'a str {
    let row = row_for(ledger, family);
    row.open_reason.as_deref().unwrap_or_default()
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

/// The ledger for one fixture, measured from the live scoped target.
fn pg_ledger(fixture: &PgLedgerFixture) -> ScopeInventory {
    scope_inventory(&InventoryInput {
        facts: InventoryFacts::Postgres(fixture.target_state()),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: None,
        peer_actions: Vec::new(),
    })
}

/// Every row has a source and a proof, a link or a gap or a local path, and honest open state.
fn assert_rows_are_well_formed(ledger: &ScopeInventory) {
    for row in &ledger.rows {
        assert!(!row.source.is_empty(), "row {} has no source", row.family);
        assert!(!row.proof.is_empty(), "row {} has no proof", row.family);
        let local = matches!(row.family, "store_file" | "kms_file" | "backup_archives");
        let linked = row.subject_link.is_some() || row.gap.is_some() || local;
        assert!(linked, "row {} has no link or gap", row.family);
        assert_eq!(row.open, row.open_reason.is_some(), "{} open", row.family);
        let gap_open = row.gap.is_none() || row.open;
        assert!(gap_open, "row {} gap not open", row.family);
    }
}

/// The four records the store measurement cannot reach stay open and say why.
fn assert_unreachable_records_stay_open(ledger: &ScopeInventory) {
    for family in [
        "erasure_journal",
        "stderr_events",
        "enabled_peers",
        "restore_routes",
    ] {
        let row = row_for(ledger, family);
        let stays_open = row.open && row.retention_action == RetentionAction::Open;
        assert!(stays_open, "{family} stays open");
        assert!(!reason(ledger, family).is_empty(), "{family} names why");
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
    assert_eq!(postgres_names, sqlite_names, "same family set");
    assert_eq!(postgres_names.len(), 35, "the pinned family set is 35 rows");
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
        assert!(postgres_names.contains(&family), "{family} missing");
    }

    assert_eq!(ledger.backend, InventoryBackend::Postgres);
    assert_eq!(serde_json::to_value(ledger.backend).unwrap(), "postgres");
    assert!(!ledger.complete, "gaps keep complete false");
    let store_file = row_for(&ledger, "store_file");
    let named = store_file.source.contains("postgres target schema");
    assert!(named, "{}", store_file.source);
    assert!(
        sqlite.unclassified.is_empty(),
        "every SQLite fixture table is named by a row: {:?}",
        sqlite.unclassified
    );

    assert_rows_are_well_formed(&ledger);
    assert_unreachable_records_stay_open(&ledger);

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
        let item = store.get_memory_item(bank, &id("item")).unwrap();
        assert_eq!(item.expect("the live item row").subject_id, subject);
        let task = store.get_task(bank, &id("task")).unwrap();
        assert_eq!(task.expect("the live task row").subject_id, subject);
        let failure = store.get_failure(bank, &id("failure-record")).unwrap();
        assert_eq!(failure.expect("the live failure row").subject_id, subject);
        let belief = store.get_belief(bank, &id("belief")).unwrap();
        assert_eq!(belief.expect("the live belief row").subject_id, subject);
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
    let mut stable = persona.stable.iter();
    let has_stable = stable.any(|entry| entry.key == fixture_id(SHARED, "stable-key"));
    assert!(has_stable, "the shared stable entry is missing");
    let mut preferences = persona.preferences.iter();
    let has_preference = preferences.any(|p| p.key == fixture_id(SHARED, "preference-key"));
    assert!(has_preference, "the shared preference is missing");

    // unknown: the erased subject survives only as a one-way hash.
    let tombstones = store.list_tombstones(&fixture.erased_bank).unwrap();
    let tombstone = tombstones.first().expect("the fixture erasure tombstone");
    assert_eq!(tombstone.request_id, fixture_id(UNKNOWN, "request"));
    let hashed = !tombstone.subject_hash.is_empty() && tombstone.subject_hash != UNKNOWN;
    assert!(hashed, "the tombstone link is hashed");
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
        let linked = row_for(&ledger, family).subject_link.is_some();
        assert!(linked, "{family} names no direct link");
    }
    for family in [
        "assoc_edges",
        "persona_stable",
        "persona_preferences",
        "tombstones",
    ] {
        let row = row_for(&ledger, family);
        let gap = row.subject_link.is_none() && row.gap.is_some();
        assert!(gap, "{family} names no gap instead of a link");
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
    let table = vector_table.as_deref();
    assert_eq!(table, Some("item_embeddings"), "the vector table");
    let mut tables = state.tables.iter();
    let listed = tables.any(|table| table.name == "item_embeddings");
    let mut counts = state.table_counts.iter();
    let counted = counts.any(|(name, _)| name == "item_embeddings");
    assert!(!listed && !counted, "the vector table was measured");

    let derived = row_for(&ledger, "derived_item_structures");
    let why = reason(&ledger, "derived_item_structures");
    assert!(derived.open, "the derived row stays open");
    // Neither the extension-dependent relation nor the SQLite-only source may
    // be claimed missing.
    assert!(!why.contains("live store facts: item_embeddings"), "{why}");
    assert!(!why.contains("live store facts: items_fts"), "{why}");
    let per_backend = "item_embeddings* is not enumerated by the read-only target measurement; \
         not individually verified on this backend";
    assert!(why.contains(per_backend), "per-backend handling missing");
    let sqlite_only = why.contains("items_fts* is a SQLite-only full-text source");
    assert!(sqlite_only, "the SQLite-only source is not named: {why}");

    // The unclassified set is reported as measured: the fixture schema names
    // every app table with a row.
    let unclassified = &ledger.unclassified;
    assert!(unclassified.is_empty(), "{unclassified:?}");
    let mut sorted = ledger.unclassified.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, ledger.unclassified, "unclassified is not sorted");
    assert!(!ledger.complete, "an open row keeps complete false");
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
            assert!(!text.contains("://"), "row {} has a URL", row.family);
            assert!(!text.contains('@'), "row {} has userinfo", row.family);
            let secrets = [userinfo.as_deref(), password.as_deref()];
            for secret in secrets.into_iter().flatten() {
                assert!(!text.contains(secret), "row {} leaked", row.family);
            }
        }
    }
    for name in &ledger.unclassified {
        let clean = !name.contains("://") && !name.contains('@');
        assert!(clean, "an unclassified name has a connection string");
    }
    eprintln!(
        "inventory_pg credential check: {} rows checked, URL carries a password: {}",
        ledger.rows.len(),
        password.is_some()
    );
}

/// The peer fact renders over Postgres facts too: a durable acknowledgement
/// closes the family row, and a peers-disabled deployment records the known
/// not-applicable mode.
#[test]
fn inventory_pg_peer_facts_render_over_postgres_facts() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let request = unique("inv-pg-peer");
    let peer = unique("inv-pg-peer-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer denial");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-1:hash:salt").expect("acknowledge");
    let peer_rows = fixture.store.load_for_request(&request);
    let mut input = InventoryInput {
        facts: InventoryFacts::Postgres(fixture.target_state()),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: Some(true),
        peer_actions: peer_rows.expect("the durable peer rows"),
    };
    let ledger = scope_inventory(&input);
    let row = row_for(&ledger, "enabled_peers");
    assert!(!row.open, "an acknowledged peer is not open");
    assert!(row.gap.is_none(), "no acknowledgement gap applies");
    let named = row.source.contains(&format!("{peer} acknowledged"));
    assert!(named, "the acknowledgement is named: {}", row.source);

    // The same durable row, written through the SQLite store and read back
    // there, renders the same peers row.
    let sqlite_fixture = LedgerFixture::create();
    begin_denial(&sqlite_fixture.store, &request, &peer).expect("begin on SQLite");
    mark_acknowledged(&sqlite_fixture.store, &request, &peer, "gen-1:hash:salt")
        .expect("acknowledge on SQLite");
    let sqlite_rows = sqlite_fixture.store.load_for_request(&request);
    let mut sqlite_input = sqlite_fixture.input(Vec::new(), Some(true));
    sqlite_input.peer_actions = sqlite_rows.expect("the SQLite peer rows");
    assert_eq!(
        row_for(&scope_inventory(&sqlite_input), "enabled_peers"),
        row,
        "both backends render the same peers row"
    );

    // The not-applicable value, proven over Postgres facts as well.
    input.peers_enabled = Some(false);
    input.peer_actions.clear();
    let disabled = scope_inventory(&input);
    let row = row_for(&disabled, "enabled_peers");
    assert!(!row.open, "the not-applicable mode is a closed fact");
    assert!(row.source.contains("not applicable"), "{}", row.source);
}
