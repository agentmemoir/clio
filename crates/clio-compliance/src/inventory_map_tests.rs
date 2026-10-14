// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-family mapping tests: the pinned
//! class of every planned family, the fixture store's exact per-family counts,
//! the link-removal rerun, the restart equality, the second-bank decoy, and
//! the known missing links that stay named omissions.
//!
//! ## Owns
//! - Every assertion about `subject_family_mapping` output
//!
//! ## Does not own
//! - The mapping itself (`inventory_map.rs`)
//! - The fixture store (`inventory_map_fixtures.rs`) and the class writers
//!   (`inventory_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the subject-family mapping tests.

use super::{
    FamilyClass, FamilySelection, SubjectFamilyInput, SubjectFamilyMapping, subject_family_mapping,
};
use crate::inventory::{InventoryBackend, InventoryFacts, InventoryInput, scope_inventory};
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, carrier, fixture_id, semantic_item};
use crate::inventory_map_fixtures::{FIXTURE_COUNTS, MapFixture};
use crate::test_unique::unique;
use clio_store::{Store, sqlite_file_state};
use clio_types::EpisodicType;

/// The pinned class of every planned family, one entry per `ROW_PLAN` row:
/// a family bound from the plan's own link, gap, and readability columns.
const CLASS_MAP: &[(&str, FamilyClass)] = &[
    ("items", FamilyClass::Selected),
    ("triples", FamilyClass::Selected),
    ("assoc_edges", FamilyClass::InseparableShared),
    ("persona_stable", FamilyClass::InseparableShared),
    ("persona_preferences", FamilyClass::InseparableShared),
    ("tasks", FamilyClass::Selected),
    ("failures", FamilyClass::Selected),
    ("beliefs", FamilyClass::Selected),
    ("tombstones", FamilyClass::UnknownLink),
    ("derived_item_structures", FamilyClass::OtherOnly),
    ("bank_structures", FamilyClass::InseparableShared),
    ("events", FamilyClass::OtherOnly),
    ("sync_state", FamilyClass::InseparableShared),
    ("subject_keys", FamilyClass::OtherOnly),
    ("persona_erase_tombstones", FamilyClass::InseparableShared),
    ("legacy_tasks", FamilyClass::InseparableShared),
    ("legacy_failures", FamilyClass::InseparableShared),
    ("legacy_tombstones", FamilyClass::InseparableShared),
    ("archive_experiments", FamilyClass::InseparableShared),
    ("copy_inventory", FamilyClass::InseparableShared),
    ("durable_requests", FamilyClass::InseparableShared),
    ("erasure_adapters", FamilyClass::InseparableShared),
    ("local_erasure_journal", FamilyClass::InseparableShared),
    ("local_events", FamilyClass::InseparableShared),
    ("restore_guards", FamilyClass::InseparableShared),
    ("subject_family_links", FamilyClass::InseparableShared),
    ("store_file", FamilyClass::NotStored),
    ("kms_file", FamilyClass::NotStored),
    ("backup_archives", FamilyClass::NotStored),
    ("export_copies", FamilyClass::NotStored),
    ("stderr_events", FamilyClass::NotStored),
    ("enabled_peers", FamilyClass::NotStored),
    ("restore_routes", FamilyClass::NotStored),
    ("erasure_journal", FamilyClass::NotStored),
];

/// One mapping over the fixture's live store file.
fn map(fixture: &MapFixture, subject_id: &str) -> SubjectFamilyMapping {
    let facts = fixture.facts();
    map_over(fixture, subject_id, &facts)
}

/// One mapping over given live-store facts for the fixture's store.
fn map_over(
    fixture: &MapFixture,
    subject_id: &str,
    facts: &InventoryFacts,
) -> SubjectFamilyMapping {
    let inventory = scope_inventory(&InventoryInput {
        facts: facts.clone(),
        paths: fixture.paths(),
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
    .expect("the mapping reads the fixture store without error")
}

/// One family's selection, or a panic when the mapping has none.
fn family<'a>(mapping: &'a SubjectFamilyMapping, name: &str) -> &'a FamilySelection {
    mapping
        .families
        .iter()
        .find(|selection| selection.family == name)
        .unwrap_or_else(|| panic!("the mapping has no {name} family"))
}

/// The selected count of every family, in report order.
fn selected_counts(mapping: &SubjectFamilyMapping) -> Vec<(&'static str, u64)> {
    mapping
        .families
        .iter()
        .map(|selection| (selection.family, selection.selected))
        .collect()
}

/// Assert no selected id and no omission text of `mapping` names another
/// subject.
fn assert_no_other_subject(mapping: &SubjectFamilyMapping) {
    for selection in &mapping.families {
        assert!(
            selection.selected_ids.iter().all(|id| !id.contains(OTHER)),
            "{} selected another subject's record: {:?}",
            selection.family,
            selection.selected_ids
        );
        assert!(
            selection
                .omission
                .as_deref()
                .is_none_or(|reason| !reason.contains(OTHER)),
            "{} names another subject: {:?}",
            selection.family,
            selection.omission
        );
    }
    assert!(
        mapping
            .omissions
            .iter()
            .all(|reason| !reason.contains(OTHER)),
        "the omission list names another subject: {:?}",
        mapping.omissions
    );
}

#[test]
fn every_planned_family_is_bound_to_a_pinned_class() {
    let fixture = MapFixture::create();
    let mapping = map(&fixture, SELECTED);

    // One selection per planned row: `CLASS_MAP` is 1:1 with `ROW_PLAN`, so
    // any plan drift breaks the equality.
    let classes: Vec<(&str, FamilyClass)> = mapping
        .families
        .iter()
        .map(|selection| (selection.family, selection.class))
        .collect();
    assert_eq!(classes, CLASS_MAP, "the pinned per-family class map");
    assert_eq!(mapping.backend, InventoryBackend::Sqlite);
    assert_eq!(mapping.subject_id, SELECTED);
}

#[test]
fn the_fixture_store_gives_exact_per_family_counts() {
    let fixture = MapFixture::create();
    let mapping = map(&fixture, SELECTED);

    let observed: Vec<(&str, u64, u64, bool)> = mapping
        .families
        .iter()
        .map(|s| (s.family, s.selected, s.omitted, s.omission.is_some()))
        .collect();
    assert_eq!(observed, FIXTURE_COUNTS, "the pinned per-family counts");
    assert_eq!(mapping.selected_total, 10);
    assert_eq!(mapping.omitted_total, 66);
    assert!(!mapping.complete, "named omissions keep the mapping open");
    for name in ["items", "triples", "tasks", "failures", "beliefs"] {
        assert!(
            family(&mapping, name).selected > 0,
            "{name} selects the subject's own records"
        );
    }
    for (name, needle) in [
        ("items", "store-wide"),
        ("derived_item_structures", "no per-subject read path"),
        ("assoc_edges", "bank-scoped"),
        ("tombstones", "one-way subject_hash"),
    ] {
        let reason = family(&mapping, name).omission.as_deref().unwrap_or("");
        assert!(reason.contains(needle), "{name}: {reason}");
    }

    // A family whose store-wide measured rows are all bound is complete.
    let mut facts = fixture.facts();
    let InventoryFacts::Sqlite(state) = &mut facts else {
        panic!("the fixture is a SQLite store");
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
fn a_removed_stored_link_and_the_shared_records_stay_named_omissions() {
    let fixture = MapFixture::create();
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
    assert_eq!(
        items_after.selected,
        family(&before, "items").selected - 1,
        "the removed stored link is no longer selected"
    );
    assert!(
        items_after.omitted > 0 && items_after.omission.is_some(),
        "the family names what it omits: {:?}",
        items_after.omission
    );

    // A subject whose only stored link is its one item: with the link gone
    // the family selects nothing and omits every measured row.
    let only = unique("inv-map-only");
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
    let without_link = map(&fixture, &only);
    let items = family(&without_link, "items");
    let measured_items = sqlite_file_state(&fixture.db_path)
        .expect("measure the fixture store")
        .count_of("items")
        .expect("the store has an items table");
    assert_eq!(items.selected, 0, "nothing is selected without a link");
    assert_eq!(
        items.omitted, measured_items,
        "every store-wide measured row is omitted"
    );
    assert!(
        items
            .omission
            .as_deref()
            .is_some_and(|reason| reason.contains("store-wide")),
        "the omission is named and says store-wide: {:?}",
        items.omission
    );

    for shared in ["assoc_edges", "persona_stable", "persona_preferences"] {
        let selection = family(&without_link, shared);
        assert_eq!(selection.class, FamilyClass::InseparableShared);
        assert_eq!(selection.selected, 0);
        assert!(
            selection.omission.is_some() && !selection.complete,
            "{shared} stays a named omission"
        );
    }

    for mapping in [&before, &after, &without_link] {
        assert_no_other_subject(mapping);
    }
}

#[test]
fn restarting_the_store_keeps_every_per_family_count() {
    let mut fixture = MapFixture::create();
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
    assert!(
        !after.complete,
        "named omissions keep the mapping incomplete"
    );
}

#[test]
fn a_second_bank_decoy_for_the_subject_changes_no_selection() {
    let fixture = MapFixture::create();
    let before = map(&fixture, SELECTED);
    // A decoy for the mapped subject in a second bank: every read is
    // bank-scoped, and item ids are globally unique, so it takes a fresh id.
    let decoy_bank = unique("inv-map-decoy-bank");
    fixture
        .store
        .create_memory_item(
            &semantic_item(&fixture_id(SELECTED, "decoy"), &decoy_bank, SELECTED),
            "harness",
        )
        .expect("write the second-bank decoy");
    let after = map(&fixture, SELECTED);

    assert_eq!(
        selected_counts(&before),
        selected_counts(&after),
        "the bank filter keeps the second bank's decoy out"
    );
    assert!(
        after.omitted_total > before.omitted_total,
        "the decoy is still measured store-wide: {} -> {}",
        before.omitted_total,
        after.omitted_total
    );
    assert_no_other_subject(&after);
}

#[test]
fn a_triple_with_a_distinct_edge_id_is_still_selected_by_its_carrier_item() {
    let fixture = MapFixture::create();
    let carrier_item = carrier(
        &fixture_id(SELECTED, "pin"),
        &fixture.bank,
        SELECTED,
        EpisodicType::Triple,
    );
    // Re-shape the fixture's own triple under a new edge id and a fresh carrier
    // item: the pinned case where the edge id differs from its carrier item
    // id. The edge id is what the family reports; the carrier item is the
    // subject link.
    let mut edge = fixture
        .store
        .list_triples(&fixture.bank)
        .expect("list the fixture triples")
        .into_iter()
        .find(|triple| triple.id == fixture_id(SELECTED, "triple"))
        .expect("the fixture's selected-subject triple");
    edge.id = fixture_id(SELECTED, "pin-edge");
    edge.item_id = carrier_item.id.clone();
    edge.predicate = "attribute-pin".into();
    assert_ne!(edge.id, edge.item_id, "the pinned distinct-edge case");
    fixture
        .store
        .commit_triple_add(&carrier_item, &edge, false, ACTOR)
        .expect("write the pinned triple");
    let mapping = map(&fixture, SELECTED);

    let triples = family(&mapping, "triples");
    assert!(
        triples.selected_ids.iter().any(|id| id == &edge.id),
        "the distinct edge id is selected: {:?}",
        triples.selected_ids
    );
    assert_no_other_subject(&mapping);
}

#[test]
fn every_known_missing_link_stays_named_and_incomplete() {
    let fixture = MapFixture::create();
    let mapping = map(&fixture, SELECTED);

    // The rows whose link this store cannot resolve per subject: the mapping
    // does not invent a read path, it names each one and stays incomplete.
    for name in ["derived_item_structures", "events", "subject_keys"] {
        let selection = family(&mapping, name);
        assert_eq!(selection.class, FamilyClass::OtherOnly);
        assert_eq!(selection.selected, 0);
        assert!(!selection.complete, "{name} stays incomplete");
        let reason = selection
            .omission
            .as_deref()
            .unwrap_or_else(|| panic!("{name} must name its omission"));
        assert!(
            reason.contains("no per-subject read path on this store"),
            "{name}: {reason}"
        );
    }

    // The id-only link slot is reported as a named gap, never written to.
    let slot = family(&mapping, "subject_family_links");
    assert_eq!(slot.class, FamilyClass::InseparableShared);
    assert!(slot.omission.is_some() && !slot.complete);
    let slot_rows = sqlite_file_state(&fixture.db_path)
        .expect("measure the fixture store")
        .count_of("subject_family_links")
        .expect("the fixture store has the link slot table");
    assert_eq!(slot_rows, 0, "the mapping writes nothing to the slot");

    // No policy approval is requested: the refusals are named omissions.
    assert!(
        mapping
            .omissions
            .iter()
            .all(|reason| !reason.contains("approval") && !reason.contains("consent")),
        "the omissions request no policy approval: {:?}",
        mapping.omissions
    );
    assert_no_other_subject(&mapping);
    assert!(!mapping.complete);
}

#[test]
fn a_missing_family_table_stays_named_and_incomplete() {
    let fixture = MapFixture::create();
    let mut facts = fixture.facts();
    let InventoryFacts::Sqlite(state) = &mut facts else {
        panic!("the fixture is a SQLite store");
    };
    state
        .table_counts
        .retain(|(name, _)| name != "task_records");

    let mapping = map_over(&fixture, SELECTED, &facts);
    let tasks = family(&mapping, "tasks");
    assert_eq!(
        tasks.selected, 1,
        "the stored task record is still selected"
    );
    assert_eq!(tasks.omitted, 0, "nothing measured, nothing omitted");
    let reason = tasks
        .omission
        .as_deref()
        .expect("a missing family table stays named");
    assert!(reason.contains("task_records"), "{reason}");
    assert!(
        !tasks.complete,
        "a missing family table keeps the family incomplete"
    );
}
