// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite subject-bundle scope tests: the
//! proof that the bundle's task and failure families count the mapped
//! subject's own history rows, and that neither family's cap is reached by
//! another subject's fuller and newer history page.
//!
//! ## Owns
//! - The bank page of another subject's rows the selected family must survive
//! - The selected family's full count and its missing cap omission
//!
//! ## Does not own
//! - The bundle itself (`subject_bundle.rs`) and the family selection
//!   (`inventory_map.rs`)
//! - The pinned fixture counts (`subject_bundle_tests.rs`) and the fixture
//!   store (`inventory_map_fixtures.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the subject-scoped history families.

use super::{SubjectBundle, SubjectFamilyInput, build_subject_bundle};
use crate::export::HISTORY_LIMIT;
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, carrier};
use crate::inventory_map_fixtures::MapFixture;
use crate::test_unique::unique;
use clio_store::HistoryStore;
use clio_types::{EpisodicType, EpistemicKind, FailureRecord, TaskRecord};

/// The selected subject's seeded rows sort older than every fixture row.
const OLDER: &str = "2025-01-01T00:00:00Z";
/// The other subject's seeded rows sort newer than every fixture row.
const NEWER: &str = "2027-01-01T00:00:00Z";
/// The selected subject's own seeded rows per history family.
const SEEDED: u64 = 2;
/// The selected subject's own task rows the fixture store already holds.
const FIXTURE_TASKS: u64 = 1;
/// The selected subject's own failure rows the fixture store already holds.
const FIXTURE_FAILURES: u64 = 1;

/// One task row for `subject` created at `created_at`.
fn seed_task(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) {
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
        definition: "fill the history page".into(),
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
}

/// One failure row for `subject` created at `created_at`.
fn seed_failure(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) {
    let item_id = unique(&format!("{tag}-item"));
    let mut item = carrier(&item_id, &fixture.bank, subject, EpisodicType::Failure);
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(clio_types::SourceType::AgentInferred);
    let failure = FailureRecord {
        context: None,
        id: unique(&format!("{tag}-record")),
        bank_id: fixture.bank.clone(),
        item_id,
        task_id: unique(&format!("{tag}-task")),
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
}

/// The fixture store's facts with each history family pinned to the selected
/// subject's own row count, so a family can only name a cap omission.
fn balanced_history_facts(fixture: &MapFixture) -> InventoryFacts {
    let mut facts = fixture.facts();
    let InventoryFacts::Sqlite(state) = &mut facts else {
        panic!("the fixture is a SQLite store");
    };
    for (name, count) in &mut state.table_counts {
        let selected = match name.as_str() {
            "task_records" => Some(FIXTURE_TASKS + SEEDED),
            "failure_records" => Some(FIXTURE_FAILURES + SEEDED),
            _ => None,
        };
        if let Some(selected) = selected {
            *count = selected;
        }
    }
    facts
}

/// One bundle for the selected subject over the given facts.
fn build(fixture: &MapFixture, facts: &InventoryFacts) -> SubjectBundle {
    build_subject_bundle(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id: SELECTED,
        facts,
        backend: InventoryBackend::Sqlite,
    })
    .expect("build the subject bundle")
}

/// One family's manifest selection of `bundle`, or a panic when it has none.
fn family_manifest<'a>(bundle: &'a SubjectBundle, name: &str) -> &'a super::FamilySelection {
    bundle
        .manifest
        .families
        .iter()
        .find(|selection| selection.family == name)
        .unwrap_or_else(|| panic!("the manifest has no {name} family"))
}

/// The selected subject's history families count its own rows, and neither
/// family's cap is reached by the other subject's fuller, newer page.
#[test]
fn the_selected_history_families_ignore_a_full_page_of_the_other_subject() {
    let fixture = MapFixture::create();
    for index in 0..SEEDED {
        seed_task(&fixture, SELECTED, &format!("sel-task-{index}"), OLDER);
        seed_failure(&fixture, SELECTED, &format!("sel-fail-{index}"), OLDER);
    }
    // One row past the cap, so a bank-wide page is full and truncated before
    // it ever reaches the selected subject's older rows.
    for index in 0..=HISTORY_LIMIT {
        seed_task(&fixture, OTHER, &format!("other-task-{index}"), NEWER);
        seed_failure(&fixture, OTHER, &format!("other-fail-{index}"), NEWER);
    }

    let bundle = build(&fixture, &balanced_history_facts(&fixture));
    for (name, expected) in [
        ("tasks", FIXTURE_TASKS + SEEDED),
        ("failures", FIXTURE_FAILURES + SEEDED),
    ] {
        let selection = family_manifest(&bundle, name);
        assert_eq!(
            selection.selected, expected,
            "{name} counts every one of the subject's own rows"
        );
        assert!(
            selection.selected_ids.iter().all(|id| !id.contains(OTHER)),
            "{name} selects no other subject's row: {:?}",
            selection.selected_ids
        );
        assert!(
            selection.omission.is_none() && selection.complete,
            "{name} is neither capped nor omitted by the other subject's page: {:?}",
            selection.omission
        );
    }

    // Over the store-wide measured counts the omitted rows are named, yet no
    // cap is: only the subject's own rows can reach it.
    let measured = build(&fixture, &fixture.facts());
    for name in ["tasks", "failures"] {
        let selection = family_manifest(&measured, name);
        assert!(
            selection
                .omission
                .as_deref()
                .is_some_and(|reason| reason.contains("store-wide")),
            "{name} names the store-wide rows it did not select: {:?}",
            selection.omission
        );
        assert!(
            selection
                .omission
                .as_deref()
                .is_none_or(|reason| !reason.contains("capped at")),
            "{name} names no cap: {:?}",
            selection.omission
        );
    }

    // The store's own subject-scoped reads are what the bundle's counts rest
    // on, so assert them directly.
    let tasks = fixture
        .store
        .list_tasks_for_subject(&fixture.bank, SELECTED, HISTORY_LIMIT)
        .expect("read the selected subject's tasks");
    assert_eq!(tasks.len() as u64, FIXTURE_TASKS + SEEDED);
    assert!(tasks.iter().all(|task| task.subject_id == SELECTED));
    let failures = fixture
        .store
        .list_failures_for_subject(&fixture.bank, SELECTED, HISTORY_LIMIT)
        .expect("read the selected subject's failures");
    assert_eq!(failures.len() as u64, FIXTURE_FAILURES + SEEDED);
    assert!(
        failures
            .iter()
            .all(|failure| failure.subject_id == SELECTED)
    );
}
