// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite tests of the blanket
//! `FamilyLinkRead` implementation: every read a subject-family mapping holds
//! returns the mapped subject's own rows, and a history cap counts those rows
//! rather than the bank's.
//!
//! ## Owns
//! - The subject restriction each blanket-implemented read applies
//! - The per-subject history cap the blanket implementation forwards
//!
//! ## Does not own
//! - The trait and its implementation (`inventory_link_read.rs`)
//! - The family mapping built on those reads (`inventory_map.rs`)
//! - The fixture store (`inventory_map_fixtures.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the blanket read implementation.

use super::FamilyLinkRead;
use crate::export::HISTORY_LIMIT;
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, carrier, fixture_id};
use crate::inventory_map_fixtures::MapFixture;
use crate::test_unique::unique;
use clio_store::HistoryStore;
use clio_types::{EpisodicType, TaskRecord};

/// One task row of `subject`, so a read can be shown to return it or not.
fn seed_task(fixture: &MapFixture, subject: &str, tag: &str) {
    let item_id = unique(&format!("{tag}-item"));
    let item = carrier(&item_id, &fixture.bank, subject, EpisodicType::Task);
    let task = TaskRecord {
        context: None,
        id: unique(&format!("{tag}-record")),
        bank_id: fixture.bank.clone(),
        item_id,
        task_id: unique(&format!("{tag}-task")),
        subject_id: subject.to_owned(),
        version: 1,
        definition: "exercise the scoped read".into(),
        steps: vec!["write".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        admission_score: 0.5,
        created_at: "2026-06-01T00:00:00Z".into(),
    };
    fixture
        .store
        .commit_task_upsert(&item, &task, ACTOR)
        .expect("write the seeded task");
}

/// Every blanket-implemented read is scoped to the named subject, and the two
/// history reads cap that subject's own rows.
#[test]
fn the_blanket_reads_are_scoped_to_the_named_subject() {
    let fixture = MapFixture::create();
    seed_task(&fixture, SELECTED, "scoped-blanket-sel");
    seed_task(&fixture, OTHER, "scoped-blanket-other");

    let items = FamilyLinkRead::list_subject_items(&fixture.store, &fixture.bank, SELECTED)
        .expect("read the subject's items");
    assert!(
        items
            .iter()
            .any(|item| item.item_id == fixture_id(SELECTED, "item")),
        "the subject's own item is read: {items:?}"
    );
    assert!(
        !items
            .iter()
            .any(|item| item.item_id == fixture_id(OTHER, "item")),
        "the other subject's own item stays out"
    );

    let tasks = FamilyLinkRead::list_tasks(&fixture.store, &fixture.bank, SELECTED, HISTORY_LIMIT)
        .expect("read the subject's tasks");
    assert_eq!(tasks.len(), 2, "the fixture row plus the seeded one");
    assert!(tasks.iter().all(|task| task.subject_id == SELECTED));

    let failures =
        FamilyLinkRead::list_failures(&fixture.store, &fixture.bank, SELECTED, HISTORY_LIMIT)
            .expect("read the subject's failures");
    assert_eq!(failures.len(), 1, "the fixture's own failure row");
    assert!(
        failures
            .iter()
            .all(|failure| failure.subject_id == SELECTED)
    );

    let beliefs = FamilyLinkRead::list_beliefs(&fixture.store, &fixture.bank)
        .expect("read the bank's beliefs");
    assert!(
        beliefs.iter().any(|belief| belief.subject_id == OTHER),
        "the belief read is bank-wide and the mapping filters it"
    );
    let triples =
        FamilyLinkRead::list_triples(&fixture.store, &fixture.bank).expect("read the triples");
    assert!(!triples.is_empty(), "the fixture store holds triples");

    // The cap counts the subject's own rows: one row is returned although the
    // bank holds a second subject's task row as well.
    let capped =
        FamilyLinkRead::list_tasks(&fixture.store, &fixture.bank, SELECTED, 1).expect("read one");
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].subject_id, SELECTED);
}
