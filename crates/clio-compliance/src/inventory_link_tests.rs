// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-link-class tests of the scope
//! ledger: the four fixture classes as written into the store, the per-class
//! link or gap classification of the SQLite ledger rows, and the link-removal
//! rerun.
//!
//! ## Owns
//! - The store-level proof that each fixture class holds the records its
//!   class is named by, including the triples carrier linkage and the erased
//!   subject's purged triple
//! - The per-class mapping from ledger rows to link or gap, proven on SQLite
//!   without a Postgres database
//! - The link-removal rerun: a removed stored link consumes a live row while
//!   the family link row stays open
//!
//! ## Does not own
//! - The fixture store (`inventory_fixture.rs`) and the pinned family set
//!   (`inventory_tests.rs`)
//! - The ledger itself (`inventory.rs`)
//!
//! ## Boundary
//!
//! Changes to how a link class is proven should normally require changes
//! here. Changes to the pinned family set or the open-reason rules should not.
//!
//! Keep this module focused on the subject-link classes.

use super::{
    Custody, InventoryRow, Owner, RetentionAction, reason, row_for, scope_inventory, write_file,
};
use crate::inventory_fixture::{LedgerFixture, OTHER, SELECTED, SHARED, UNKNOWN, fixture_id};
use clio_store::{
    BeliefStore, ComplianceStore, HistoryStore, PersonaStore, Store, sqlite_file_state,
};
use clio_types::EpisodicType;

#[test]
fn each_subject_link_class_is_proven_by_a_named_fixture() {
    let fixture = LedgerFixture::create();
    let store = &fixture.store;
    let bank = fixture.bank.as_str();

    // selected and other: four records each with a direct `subject_id` link.
    for subject in [SELECTED, OTHER] {
        let id = |role: &str| fixture_id(subject, role);
        let item = store.get_memory_item(bank, &id("item")).unwrap().unwrap();
        assert_eq!(item.subject_id, subject);
        let task = store.get_task(bank, &id("task")).unwrap().unwrap();
        assert_eq!(task.subject_id, subject);
        let failure = store
            .get_failure(bank, &id("failure-record"))
            .unwrap()
            .unwrap();
        assert_eq!(failure.subject_id, subject);
        let belief = store.get_belief(bank, &id("belief")).unwrap().unwrap();
        assert_eq!(belief.subject_id, subject);
        let triple = store
            .get_triple(bank, &id("triple"))
            .unwrap()
            .expect("the fixture triple");
        let triple_carrier = store
            .get_memory_item(bank, &triple.item_id)
            .unwrap()
            .expect("the fixture triple's carrier item");
        assert_eq!(triple.item_id, id("triple"));
        assert_eq!(triple_carrier.subject_id, subject);
        assert_eq!(
            triple_carrier.episodic_type,
            Some(EpisodicType::Triple),
            "the triple's subject link runs through the carrier item"
        );
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
    let shared_triple = store
        .get_triple(bank, &fixture_id(SHARED, "triple"))
        .unwrap()
        .expect("the fixture shared triple");
    assert_eq!(
        shared_triple.subject, SHARED,
        "the SPO subject is the shared marker, never a data-subject id"
    );

    // unknown: the erased subject survives only as a one-way hash.
    let tombstones = store.list_tombstones(&fixture.erased_bank).unwrap();
    let tombstone = tombstones.first().expect("the fixture erasure tombstone");
    assert_eq!(tombstone.request_id, fixture_id(UNKNOWN, "request"));
    assert!(
        !tombstone.subject_hash.is_empty() && tombstone.subject_hash != UNKNOWN,
        "the tombstone link is hashed, never the subject id"
    );
    assert!(
        store
            .get_triple(&fixture.erased_bank, &fixture_id(UNKNOWN, "triple"))
            .unwrap()
            .is_none(),
        "the erasure purged the erased subject's triple"
    );
}

#[test]
fn each_link_class_maps_to_its_ledger_rows_link_or_gap() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), None));

    // selected and other: the direct-link families carry their subject link,
    // and the triples family links through its carrier item.
    assert_eq!(
        row_for(&ledger, "items").subject_link.as_deref(),
        Some("items.subject_id")
    );
    for family in ["items", "tasks", "failures", "beliefs", "triples"] {
        let row = row_for(&ledger, family);
        assert!(
            row.subject_link.is_some(),
            "{family} names its subject link"
        );
    }
    assert_eq!(
        row_for(&ledger, "triples").subject_link.as_deref(),
        Some("triples.item_id -> items.subject_id")
    );
    assert!(
        reason(&ledger, "triples").contains("the SPO subject is not a data-subject id"),
        "the triples row also names the no-subject gap: {}",
        reason(&ledger, "triples")
    );

    // shared: rows that name no subject at all stay open as gap rows.
    for family in ["assoc_edges", "persona_stable", "persona_preferences"] {
        let row = row_for(&ledger, family);
        assert!(
            row.subject_link.is_none() && row.gap.is_some() && row.open,
            "{family} is a shared, no-subject gap row"
        );
    }

    // unknown: the tombstone family names the one-way hash, never the id.
    let tombstones = row_for(&ledger, "tombstones");
    assert!(tombstones.subject_link.is_none() && tombstones.open);
    assert!(
        reason(&ledger, "tombstones").contains("one-way subject_hash"),
        "{}",
        reason(&ledger, "tombstones")
    );
}

#[test]
fn removing_a_stored_subject_link_keeps_the_family_link_row_open() {
    let fixture = LedgerFixture::create();
    let before = scope_inventory(&fixture.input(Vec::new(), Some(false)));
    let link_before = row_for(&before, "items").clone();
    assert!(
        link_before.open && link_before.gap.is_some(),
        "the family link row starts open with a named gap"
    );
    let items_before = sqlite_file_state(&fixture.db_path)
        .expect("measure the store before the removal")
        .count_of("items")
        .expect("the store has an items table");

    // Remove one stored subject link through the narrowest public path.
    let removed = fixture
        .store
        .delete_item(&fixture.bank, &fixture_id(SELECTED, "item"))
        .expect("remove one stored item row");
    assert!(removed, "the fixture item must exist before the removal");
    assert!(
        fixture
            .store
            .get_memory_item(&fixture.bank, &fixture_id(SELECTED, "item"))
            .unwrap()
            .is_none(),
        "the stored subject link is gone"
    );
    let items_after = sqlite_file_state(&fixture.db_path)
        .expect("measure the store after the removal")
        .count_of("items")
        .expect("the store has an items table");
    assert!(
        items_after < items_before,
        "the removal consumes one stored items row: {items_before} -> {items_after}"
    );

    // Declare one caller-owned copy beside the store, then repeat.
    let copy = fixture.caller_path("declared-copy.json");
    write_file(&copy, "caller-held export bytes");
    let after = scope_inventory(&fixture.input(vec![copy.clone()], Some(false)));

    let link_after = row_for(&after, "items");
    assert_eq!(
        link_after, &link_before,
        "the family link row stays open, with the same owner and action"
    );
    let others_before: Vec<&InventoryRow> = before
        .rows
        .iter()
        .filter(|row| row.family != "export_copies")
        .collect();
    let others_after: Vec<&InventoryRow> = after
        .rows
        .iter()
        .filter(|row| row.family != "export_copies")
        .collect();
    assert_eq!(
        others_before, others_after,
        "the open set is structural: removing one stored link changes no other row"
    );

    let copy_row = row_for(&after, "export_copies");
    assert_eq!(copy_row.custody, Custody::Caller);
    assert_eq!(copy_row.owner, Owner::Operator);
    assert_eq!(copy_row.retention_action, RetentionAction::Record);
    assert!(copy_row.open, "a declared copy stays open");
    assert!(copy_row.source.contains(&copy.display().to_string()));
    assert!(
        reason(&after, "export_copies").contains("declared caller copy"),
        "{}",
        reason(&after, "export_copies")
    );
    assert!(
        !after.complete,
        "whole-scope completion stays false after the rerun"
    );
    assert!(copy.exists(), "Clio never deletes a declared copy");
}
