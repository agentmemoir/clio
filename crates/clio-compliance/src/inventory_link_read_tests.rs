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
use clio_store::HistoryStore;
use clio_types::{EpisodicType, EpistemicKind, FailureRecord, TaskRecord};

/// Creation time of the subject's own seeded rows: newer than the fixture's rows, so a capped read of the subject returns the seeded one.
const SUBJECT_AT: &str = "2027-01-01T00:00:00Z";
/// Creation time of the other subject's seeded rows: strictly newer than the subject's and the fixture's, so a capped read that spans the bank returns one of these instead.
const OTHER_AT: &str = "2028-01-01T00:00:00Z";

/// Write one task row for `subject` created at `created_at`, named by `tag`, and return its task id.
fn seed_task(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) -> String {
    let item_id = format!("{tag}-item");
    let mut item = carrier(&item_id, &fixture.bank, subject, EpisodicType::Task);
    item.created_at = created_at.to_owned();
    let task = TaskRecord {
        context: None,
        id: format!("{tag}-record"),
        bank_id: fixture.bank.clone(),
        item_id,
        task_id: tag.to_owned(),
        subject_id: subject.to_owned(),
        version: 1,
        definition: "exercise the scoped read".into(),
        steps: vec!["write".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        admission_score: 0.5,
        created_at: created_at.to_owned(),
    };
    fixture
        .store
        .commit_task_upsert(&item, &task, ACTOR)
        .expect("write the seeded task");
    tag.to_owned()
}

/// Write one failure row for `subject` created at `created_at`, named by `tag`, and return its id.
fn seed_failure(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) -> String {
    let item_id = format!("{tag}-item");
    let mut item = carrier(&item_id, &fixture.bank, subject, EpisodicType::Failure);
    item.created_at = created_at.to_owned();
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(clio_types::SourceType::AgentInferred);
    let failure = FailureRecord {
        context: None,
        id: tag.to_owned(),
        bank_id: fixture.bank.clone(),
        item_id,
        task_id: format!("{tag}-task"),
        subject_id: subject.to_owned(),
        attempt_n: 1,
        what_failed: "compile error".into(),
        lesson: "run the check first".into(),
        lesson_tokens: 5,
        evidence_ref: None,
        admission_score: 0.6,
        epistemic_kind: EpistemicKind::Belief,
        created_at: created_at.to_owned(),
    };
    fixture
        .store
        .commit_failure_record(&item, &failure, ACTOR)
        .expect("write the seeded failure");
    tag.to_owned()
}

/// Every blanket-implemented read is scoped to the named subject, and the two
/// history reads cap that subject's own rows.
///
/// The other subject's rows carry a strictly newer `created_at` than the subject's own, so a read that ignores the subject returns the other subject's row from a cap of one; their ids also sort ahead of the subject's on the ordering's id tiebreak.
#[test]
fn the_blanket_reads_are_scoped_to_the_named_subject() {
    let fixture = MapFixture::create();
    let selected_task = seed_task(&fixture, SELECTED, "sel-task", SUBJECT_AT);
    seed_task(&fixture, OTHER, "other-task", OTHER_AT);
    let selected_failure = seed_failure(&fixture, SELECTED, "sel-failure", SUBJECT_AT);
    seed_failure(&fixture, OTHER, "other-failure", OTHER_AT);

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
    assert_eq!(failures.len(), 2, "the fixture row plus the seeded one");
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

    // A cap of one while the bank also holds a newer task row of the other
    // subject: the row returned is the subject's own seeded task.
    let capped =
        FamilyLinkRead::list_tasks(&fixture.store, &fixture.bank, SELECTED, 1).expect("read one");
    assert_eq!(capped.len(), 1);
    assert_eq!(
        (capped[0].subject_id.as_str(), capped[0].task_id.as_str()),
        (SELECTED, selected_task.as_str()),
        "the capped task is the subject's own seeded row, not the newer one"
    );

    // The same for a cap of one failure row.
    let capped_failures = FamilyLinkRead::list_failures(&fixture.store, &fixture.bank, SELECTED, 1)
        .expect("read one failure");
    assert_eq!(capped_failures.len(), 1);
    assert_eq!(
        (
            capped_failures[0].subject_id.as_str(),
            capped_failures[0].id.as_str()
        ),
        (SELECTED, selected_failure.as_str()),
        "the capped failure is the subject's own seeded row, not the newer one"
    );
}
