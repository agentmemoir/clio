// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the subject-family
//! mapping tests: the same pinned class map and fixture writes as the SQLite
//! slice, read from a real search-path-scoped store, plus the stored-link
//! removal boundary, the balanced complete family, the missing planned
//! table, the restart, and the named missing-link evidence.
//!
//! ## Owns
//! - `MapPgFixture`: the private-schema store and restart KMS Arc
//! - Every assertion about a Postgres mapping, pinned in one `PINNED` table
//!
//! ## Does not own
//! - The mapping (`inventory_map.rs`) and the class writers and lock
//!   (`inventory_pg_support_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Changes to what the mapping must prove on Postgres belong here; the
//! mapping or the row plan should not require changes here.
//!
//! Keep this module focused on the live-Postgres subject-family mapping tests.

use super::super::ROW_PLAN;
use super::{
    FamilyClass, FamilySelection, SubjectFamilyInput, SubjectFamilyMapping, subject_family_mapping,
};
use crate::inventory::{InventoryBackend, InventoryFacts, InventoryInput, scope_inventory};
use crate::inventory_fixture::{OTHER, SELECTED, fixture_id};
use crate::inventory_pg_support_tests::{
    ledger_paths, pg_checked_url, pg_skip_note, semantic_item, write_direct_link_class,
    write_hashed_link_class, write_shared_class,
};
use crate::test_unique::unique;
use clio_store::{
    DekProvider, LocalDevKms, PgTestGuard, PostgresStore, ScopedSchema, Store, lock_pg_test,
    postgres_target_state, probe_postgres,
};
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Every planned family's live-fixture pin, in row-plan order:
/// (family, class, selected, SQLite selected, omitted, has an omission).
#[rustfmt::skip]
const PINNED: &[(&str, FamilyClass, u64, u64, u64, bool)] = &[
    ("items", FamilyClass::Selected, 5, 5, 6, true),
    ("triples", FamilyClass::Selected, 2, 2, 1, true),
    ("assoc_edges", FamilyClass::InseparableShared, 0, 0, 1, true),
    ("persona_stable", FamilyClass::InseparableShared, 0, 0, 1, true),
    ("persona_preferences", FamilyClass::InseparableShared, 0, 0, 1, true),
    ("tasks", FamilyClass::Selected, 1, 1, 1, true),
    ("failures", FamilyClass::Selected, 1, 1, 1, true),
    ("beliefs", FamilyClass::Selected, 1, 1, 3, true),
    ("tombstones", FamilyClass::UnknownLink, 0, 0, 2, true),
    ("derived_item_structures", FamilyClass::OtherOnly, 0, 0, 9, true),
    ("bank_structures", FamilyClass::InseparableShared, 0, 0, 6, true),
    ("events", FamilyClass::OtherOnly, 0, 0, 16, true),
    ("sync_state", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("subject_keys", FamilyClass::OtherOnly, 0, 0, 4, true),
    ("persona_erase_tombstones", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("legacy_tasks", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("legacy_failures", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("legacy_tombstones", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("archive_experiments", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("copy_inventory", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("durable_requests", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("erasure_adapters", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("local_erasure_journal", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("local_events", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("restore_guards", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("subject_family_links", FamilyClass::InseparableShared, 0, 0, 0, true),
    ("store_file", FamilyClass::NotStored, 0, 0, 0, true),
    ("kms_file", FamilyClass::NotStored, 0, 0, 0, true),
    ("backup_archives", FamilyClass::NotStored, 0, 0, 0, true),
    ("export_copies", FamilyClass::NotStored, 0, 0, 0, true),
    ("stderr_events", FamilyClass::NotStored, 0, 0, 0, true),
    ("enabled_peers", FamilyClass::NotStored, 0, 0, 0, true),
    ("restore_routes", FamilyClass::NotStored, 0, 0, 0, true),
    ("erasure_journal", FamilyClass::NotStored, 0, 0, 0, true),
];

/// The live fixture, or a printed credential-safe skip note plus `None`.
fn map_fixture_or_skip() -> Option<(PgTestGuard, MapPgFixture)> {
    let pair = open_map_pg_fixture();
    if let Err(err) = &pair {
        eprintln!("{}", pg_skip_note("inventory map Postgres tests", err));
    }
    pair.ok()
}

/// One mapping over the fixture's live scoped target.
fn map(fixture: &MapPgFixture, subject_id: &str) -> SubjectFamilyMapping {
    let facts = fixture.facts();
    map_over(fixture, subject_id, &facts)
}

/// One mapping over facts a test may have adjusted in its own copy.
fn map_over(
    fixture: &MapPgFixture,
    subject_id: &str,
    facts: &InventoryFacts,
) -> SubjectFamilyMapping {
    let inventory = scope_inventory(&InventoryInput {
        facts: facts.clone(),
        // Placeholder paths: Postgres holds no store file and the KMS is in memory.
        paths: ledger_paths(&std::env::temp_dir().join(&fixture.bank)),
        declared_copies: Vec::new(),
        peers_enabled: Some(false),
    });
    subject_family_mapping(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id,
        facts,
        inventory: &inventory,
    })
    .expect("the mapping reads the live Postgres store without error")
}

/// One family's selection, or a panic when the mapping has none.
fn family<'a>(mapping: &'a SubjectFamilyMapping, name: &str) -> &'a FamilySelection {
    mapping
        .families
        .iter()
        .find(|selection| selection.family == name)
        .unwrap_or_else(|| panic!("the mapping has no {name} family"))
}

/// Assert nothing in `mapping` names another subject.
fn assert_no_other_subject(mapping: &SubjectFamilyMapping) {
    let named = mapping.omissions.join("; ");
    for selection in &mapping.families {
        assert!(
            selection.selected_ids.iter().all(|id| !id.contains(OTHER))
                && selection
                    .omission
                    .as_deref()
                    .is_none_or(|reason| !reason.contains(OTHER)),
            "{} leaks another subject: {:?} {:?}",
            selection.family,
            selection.selected_ids,
            selection.omission
        );
    }
    assert!(
        !named.contains(OTHER),
        "the omission list names another subject: {named:?}"
    );
}

/// A private-schema store holding one row class per subject-link class, the
/// KMS Arc a restart reopens under, and the ledger paths one mapping needs.
pub(crate) struct MapPgFixture {
    /// The store the fixture rows were written through; declared first so it
    /// drops, releasing its connections, before the schema below is dropped.
    pub(crate) store: PostgresStore,
    /// The key provider the store is sealed under; a restart reopens under it.
    pub(crate) kms: Arc<dyn DekProvider>,
    /// Bank holding the selected, other, and shared classes.
    pub(crate) bank: String,
    /// The private schema; dropped after the store.
    schema: ScopedSchema,
}

impl MapPgFixture {
    /// Close the store and reopen the same scoped schema under the same KMS.
    pub(crate) fn restart(&mut self) {
        self.store = PostgresStore::open(self.schema.url(), Arc::clone(&self.kms))
            .expect("reopen the fixture store on the same scoped schema");
    }

    /// The live-store facts measured from the scoped target at call time.
    #[must_use]
    pub(crate) fn facts(&self) -> InventoryFacts {
        InventoryFacts::Postgres(
            postgres_target_state(self.schema.url()).expect("measure the scoped Postgres target"),
        )
    }
}

/// Open the live fixture: the shared-Postgres lock first, then a private
/// schema and a store holding one row class per subject-link class, sealed
/// under a KMS this fixture owns so a restart reopens with the same Arc.
/// The guard goes to the caller so the lock spans the caller's whole test;
/// an unreachable database yields `Err` for [`pg_skip_note`], a misconfigured
/// URL panics as a harness fault, a reachable database that cannot hold the
/// fixture rows panics, and the live marker line is what a skip never prints.
pub(crate) fn open_map_pg_fixture() -> Result<(PgTestGuard, MapPgFixture), AmError> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    probe_postgres(&base)
        .map_err(|detail| AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}")))?;
    let schema = ScopedSchema::new(&base);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store =
        PostgresStore::open(schema.url(), Arc::clone(&kms)).expect("open the scoped fixture store");
    let bank = unique("inv-map-pg-bank");
    let erased_bank = unique("inv-map-pg-erased-bank");
    write_direct_link_class(&store, &bank, SELECTED);
    write_direct_link_class(&store, &bank, OTHER);
    write_shared_class(&store, &bank);
    crate::inventory_pg_tests::triples::write_triple_class(&store, &bank, &erased_bank);
    write_hashed_link_class(&store, &erased_bank);
    let state = postgres_target_state(schema.url()).expect("measure the scoped Postgres target");
    eprintln!(
        "inventory_map_pg live: schema {} holds {} app tables ({} items rows)",
        state.schema,
        state.table_count(),
        state.count_of("items").unwrap_or(0)
    );
    let fixture = MapPgFixture {
        store,
        kms,
        bank,
        schema,
    };
    Ok((guard, fixture))
}

#[test]
fn inventory_map_pg_every_planned_family_is_bound_to_a_pinned_class() {
    let Some((_guard, fixture)) = map_fixture_or_skip() else {
        return;
    };
    let mapping = map(&fixture, SELECTED);

    let planned: Vec<&str> = ROW_PLAN.iter().map(|plan| plan.family).collect();
    let mapped: Vec<&str> = mapping.families.iter().map(|s| s.family).collect();
    assert_eq!(mapped, planned, "one selection per planned row, in order");
    for (selection, (name, class, ..)) in mapping.families.iter().zip(PINNED) {
        assert_eq!(selection.family, *name);
        assert_eq!(selection.class, *class, "{name} class");
    }
    assert_eq!(mapping.backend, InventoryBackend::Postgres);
    assert_eq!(mapping.subject_id, SELECTED);
}

#[test]
fn inventory_map_pg_the_fixture_store_gives_exact_per_family_counts() {
    // The items family's omission: the accounting delta, measured counts.
    const ITEMS_OMISSION: &str = "store-wide 11 items rows measured: 5 selected for this subject, 6 not selected by this mapping";
    let Some((_guard, fixture)) = map_fixture_or_skip() else {
        return;
    };
    let mapping = map(&fixture, SELECTED);
    assert_eq!(mapping.families.len(), PINNED.len(), "one pin per family");
    for (selection, (name, _, selected, sqlite, omitted, omission)) in
        mapping.families.iter().zip(PINNED)
    {
        assert_eq!(selection.family, *name);
        assert_eq!(selection.selected, *selected, "{name} selected");
        assert_eq!(
            selection.selected, *sqlite,
            "{name} must select exactly the SQLite count"
        );
        assert_eq!(selection.omitted, *omitted, "{name} omitted");
        assert_eq!(selection.omission.is_some(), *omission, "{name} omission");
    }
    let reason = family(&mapping, "items").omission.as_deref();
    assert_eq!(reason, Some(ITEMS_OMISSION));
    assert_eq!((mapping.selected_total, mapping.omitted_total), (10, 52));
    assert!(!mapping.complete, "named omissions keep the mapping open");
    assert_no_other_subject(&mapping);

    // A family whose measured rows are all bound is complete; counts are a copy.
    let mut facts = fixture.facts();
    let InventoryFacts::Postgres(state) = &mut facts else {
        panic!("the fixture facts are Postgres");
    };
    for (name, count) in &mut state.table_counts {
        if name == "task_records" {
            *count = 1;
        }
    }
    let balanced = map_over(&fixture, SELECTED, &facts);
    let tasks = family(&balanced, "tasks");
    assert_eq!((tasks.selected, tasks.omitted), (1, 0));
    assert!(tasks.omission.is_none() && tasks.complete);
}

#[test]
fn inventory_map_pg_a_removed_stored_link_and_the_shared_records_stay_named_omissions() {
    let Some((_guard, fixture)) = map_fixture_or_skip() else {
        return;
    };
    let before = map(&fixture, SELECTED);
    assert!(
        family(&before, "items").selected > 0,
        "the selected subject's direct links are bound"
    );

    // Remove one stored subject link through the narrowest public path.
    let removed = fixture
        .store
        .delete_item(&fixture.bank, &fixture_id(SELECTED, "item"))
        .expect("remove one stored item row");
    assert!(removed, "the fixture item must exist before the removal");
    let after = map(&fixture, SELECTED);
    let items_after = family(&after, "items");
    let items_before = family(&before, "items");
    assert_eq!(
        items_after.selected,
        items_before.selected - 1,
        "the removed stored link is no longer selected"
    );
    let reason = items_after.omission.as_deref().unwrap_or_default();
    assert!(
        items_after.omitted > 0 && reason.contains("store-wide"),
        "the family names what it omits: {reason:?}"
    );

    // The inseparable shared records are named omissions, never identity-scanned.
    for shared in ["assoc_edges", "persona_stable", "persona_preferences"] {
        let selection = family(&after, shared);
        assert_eq!(selection.class, FamilyClass::InseparableShared);
        assert_eq!(selection.selected, 0);
        assert!(
            selection.omission.is_some() && !selection.complete,
            "{shared} stays a named omission"
        );
    }

    // A subject whose only stored link is its one item: link gone, nothing
    // is selected and every measured row is omitted.
    let only = unique("inv-map-pg-only");
    let only_item = fixture_id(&only, "item");
    fixture
        .store
        .create_memory_item(&semantic_item(&only_item, &fixture.bank, &only), "harness")
        .expect("write the one-item subject");
    assert_eq!(family(&map(&fixture, &only), "items").selected, 1);
    fixture
        .store
        .delete_item(&fixture.bank, &only_item)
        .expect("remove the only stored link");
    let facts = fixture.facts();
    let without_link = map_over(&fixture, &only, &facts);
    let items = family(&without_link, "items");
    let InventoryFacts::Postgres(state) = &facts else {
        panic!("the fixture facts are Postgres");
    };
    let measured = state.count_of("items").expect("items counted");
    assert_eq!(items.selected, 0, "nothing is selected without a link");
    assert_eq!(items.omitted, measured, "every measured row is omitted");
    assert!(
        items
            .omission
            .as_deref()
            .is_some_and(|reason| reason.contains("store-wide")),
        "the omission is named and says store-wide: {:?}",
        items.omission
    );

    assert_no_other_subject(&after);
    assert_no_other_subject(&without_link);
}

#[test]
fn inventory_map_pg_restarting_the_store_keeps_every_per_family_count() {
    let Some((_guard, mut fixture)) = map_fixture_or_skip() else {
        return;
    };
    let before = map(&fixture, SELECTED);
    fixture.restart();
    let after = map(&fixture, SELECTED);

    assert_eq!(
        before.families, after.families,
        "every per-family selection survives a restart"
    );
    assert_eq!(before.selected_total, after.selected_total);
    assert_eq!(before.omitted_total, after.omitted_total);
    assert_eq!(before.complete, after.complete);
    assert!(!after.complete, "named omissions keep it incomplete");
}

#[test]
fn inventory_map_pg_every_known_missing_link_stays_named_and_incomplete() {
    let Some((_guard, fixture)) = map_fixture_or_skip() else {
        return;
    };
    let mapping = map(&fixture, SELECTED);

    // The rows whose link this store cannot resolve per subject are named
    // and incomplete; the mapping invents no read path.
    for name in ["derived_item_structures", "events", "subject_keys"] {
        let selection = family(&mapping, name);
        assert!(!selection.complete, "{name} stays incomplete");
        let Some(reason) = selection.omission.as_deref() else {
            panic!("{name} has no omission")
        };
        assert!(
            reason.contains("no per-subject read path on this store"),
            "{name}: {reason}"
        );
    }

    // The id-only link slot is a named gap the mapping never writes to.
    let slot = family(&mapping, "subject_family_links");
    assert_eq!(slot.class, FamilyClass::InseparableShared);
    assert_eq!((slot.selected, slot.omitted), (0, 0));
    assert!(slot.omission.is_some() && !slot.complete);
    let slot_rows = fixture
        .schema
        .query_value("SELECT count(*)::text FROM subject_family_links")
        .expect("read the slot table's row count")
        .expect("the scoped schema has the slot table");
    assert_eq!(slot_rows, "0", "the mapping writes nothing to the slot");

    // No policy approval is requested, and no other subject is disclosed.
    let asks = mapping
        .omissions
        .iter()
        .any(|reason| reason.contains("approval") || reason.contains("consent"));
    assert!(!asks, "the omissions request no policy approval");
    assert_no_other_subject(&mapping);
    assert!(!mapping.complete);
}

#[test]
fn inventory_map_pg_a_missing_family_table_stays_named_and_incomplete() {
    let Some((_guard, fixture)) = map_fixture_or_skip() else {
        return;
    };
    // A copy of the live target state with one selected family's table dropped.
    let mut facts = fixture.facts();
    let InventoryFacts::Postgres(state) = &mut facts else {
        panic!("the fixture facts are Postgres");
    };
    state.table_counts.retain(|t| t.0 != "task_records");

    let mapping = map_over(&fixture, SELECTED, &facts);
    let tasks = family(&mapping, "tasks");
    assert_eq!(
        tasks.selected, 1,
        "the stored task record is still selected"
    );
    assert_eq!(tasks.omitted, 0, "nothing measured, nothing omitted");
    let Some(reason) = tasks.omission.as_deref() else {
        panic!("a missing family table stays named")
    };
    assert!(reason.contains("task_records"), "{reason}");
    assert!(
        !tasks.complete,
        "a missing family table keeps the family incomplete"
    );
}
