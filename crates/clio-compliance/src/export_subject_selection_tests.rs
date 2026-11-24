// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-selection payload tests: the
//! refused calls that lack measured facts or a store backend, the delivered
//! payload's manifest counts, and every narrowing omission the manifest
//! records.
//!
//! ## Owns
//! - The refusal error codes and messages of `build_subject_selection`
//! - The default-filter manifest's delivered counts and payload bytes
//! - Every narrowing omission the subject manifest records
//! - The subject-scoped history cap the delivered task and failure rows obey
//!
//! ## Does not own
//! - The payload build itself (`super`)
//! - The fixture store (`inventory_map_fixtures.rs`)
//! - The custody delivery sequence (`export_subject_scenarios_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the subject-selection payload's behavior.

use super::build_subject_selection;
use crate::bundle::{BundleEnvelope, ExportContentMode};
use crate::export::{ExportArgs, ExportFilter, HISTORY_LIMIT};
use crate::export_subject::ExportPayload;
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, carrier};
use crate::inventory_map_fixtures::MapFixture;
use crate::test_unique::unique;
use clio_store::HistoryStore;
use clio_types::{
    Actor, Bank, EpisodicType, EpistemicKind, ErrorCode, FailureRecord, OpContext, TaskRecord,
};
use std::collections::HashSet;

/// The time the tests stamp into every manifest.
const NOW: &str = "2026-01-01T00:00:00Z";

/// The context the tests pass: one harness actor over the fixture's bank.
fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), Actor::Harness)
}

/// The export arguments over the fixture's store views, without facts yet.
fn args<'a>(
    fixture: &'a MapFixture,
    context: &'a OpContext,
    filter: ExportFilter,
    facts: Option<&'a InventoryFacts>,
    backend: Option<InventoryBackend>,
) -> ExportArgs<'a> {
    ExportArgs {
        store: &fixture.store,
        compliance: &fixture.store,
        persona: &fixture.store,
        history: &fixture.store,
        belief: &fixture.store,
        ctx: context,
        destination: "unused: the payload build never touches the destination",
        filter,
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: NOW,
        subject_id: Some(SELECTED),
        facts,
        backend,
        delivery: None,
        readable_export_opt_in: true,
        recipient: Some(crate::export_tests::test_recipient()),
    }
}

/// The selected subject's seeded rows sort older than the fixture's own rows.
const OLDER: &str = "2025-01-01T00:00:00Z";
/// The other subject's seeded rows sort newer than every fixture row.
const NEWER: &str = "2027-01-01T00:00:00Z";

/// One task row for `subject` created at `created_at`, returning its task id.
fn seed_task(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) -> String {
    let item_id = unique(&format!("{tag}-item"));
    let task_id = unique(&format!("{tag}-task"));
    let item = carrier(&item_id, &fixture.bank, subject, EpisodicType::Task);
    let task = TaskRecord {
        context: None,
        id: unique(&format!("{tag}-record")),
        bank_id: fixture.bank.clone(),
        item_id,
        task_id: task_id.clone(),
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
    task_id
}

/// One failure row for `subject` created at `created_at`, returning its id.
fn seed_failure(fixture: &MapFixture, subject: &str, tag: &str, created_at: &str) -> String {
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
    failure.id
}

/// The delivered payload for the selected subject under the default filter.
fn selected_payload(fixture: &MapFixture) -> ExportPayload {
    let facts = fixture.facts();
    let context = ctx(&fixture.bank);
    build_subject_selection(
        &args(
            fixture,
            &context,
            ExportFilter::default(),
            Some(&facts),
            Some(InventoryBackend::Sqlite),
        ),
        &fixture.bank,
        SELECTED,
    )
    .expect("the default selection delivers")
}

/// The bundle envelope the payload bytes carry.
fn envelope(payload: &ExportPayload) -> BundleEnvelope {
    serde_json::from_slice(&payload.bytes).expect("the payload bytes are one bundle envelope")
}

/// A selection without measured facts is refused with the argument error.
#[test]
fn a_subject_selection_without_measured_facts_is_refused() {
    let fixture = MapFixture::create();
    let context = ctx(&fixture.bank);
    let Err(error) = build_subject_selection(
        &args(
            &fixture,
            &context,
            ExportFilter::default(),
            None,
            Some(InventoryBackend::Sqlite),
        ),
        &fixture.bank,
        SELECTED,
    ) else {
        panic!("missing facts must refuse the selection")
    };
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("measured live-store facts"),
        "the error names the missing facts"
    );
}

/// A selection without a store backend is refused with the argument error.
#[test]
fn a_subject_selection_without_a_backend_is_refused() {
    let fixture = MapFixture::create();
    let facts = fixture.facts();
    let context = ctx(&fixture.bank);
    let Err(error) = build_subject_selection(
        &args(
            &fixture,
            &context,
            ExportFilter::default(),
            Some(&facts),
            None,
        ),
        &fixture.bank,
        SELECTED,
    ) else {
        panic!("missing backend must refuse the selection")
    };
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("store backend"),
        "the error names the missing backend"
    );
}

/// The default-filter selection delivers the subject's own records only, and
/// the manifest counts name the subject mapping's selected count.
#[test]
fn the_default_selection_delivers_only_the_selected_subjects_records() {
    let fixture = MapFixture::create();
    let facts = fixture.facts();
    let context = ctx(&fixture.bank);
    let payload = build_subject_selection(
        &args(
            &fixture,
            &context,
            ExportFilter::default(),
            Some(&facts),
            Some(InventoryBackend::Sqlite),
        ),
        &fixture.bank,
        SELECTED,
    )
    .expect("the default selection delivers");
    let items = payload
        .manifest
        .counts_by_type
        .iter()
        .find(|(name, _)| name == "items")
        .expect("the manifest carries the items count");
    assert_eq!(
        items.1, 5,
        "the manifest count is the selected mapping's count"
    );
    let text = String::from_utf8(payload.bytes).expect("the payload bytes are text");
    assert!(text.contains(SELECTED), "the payload carries the subject");
    assert!(
        !text.contains(OTHER),
        "no other subject's record enters the payload"
    );
}

/// A selection under every narrowing records each narrowing it applied, so
/// the manifest is not complete.
#[test]
fn the_manifest_records_every_narrowing_the_selection_applies() {
    let fixture = MapFixture::create();
    let context = ctx(&fixture.bank);
    let facts = fixture.facts();
    let filter = ExportFilter {
        category: Some("persona".to_owned()),
        time_start: Some("2020-01-01T00:00:00Z".to_owned()),
        time_end: Some("2027-01-01T00:00:00Z".to_owned()),
        include_discarded: false,
        include_archived: false,
        ..ExportFilter::default()
    };
    let payload = build_subject_selection(
        &args(
            &fixture,
            &context,
            filter,
            Some(&facts),
            Some(InventoryBackend::Sqlite),
        ),
        &fixture.bank,
        SELECTED,
    )
    .expect("the narrowed selection delivers");
    for note in [
        "category filter narrowed items",
        "time range narrowed items",
        "discarded items excluded",
        "archived items excluded",
    ] {
        assert!(
            payload
                .manifest
                .omissions
                .iter()
                .any(|omission| omission == note),
            "the manifest records the applied narrowing: {note}"
        );
    }
    assert!(
        !payload.manifest.complete,
        "a narrowed selection is not complete"
    );
    assert_eq!(
        payload.manifest.filter["subject_id"], SELECTED,
        "the manifest names the selected subject"
    );
}

/// The task read's cap counts the selected subject's own rows, so another
/// subject's newer rows cannot push the selected subject's tasks out.
#[test]
fn every_selected_subject_task_survives_a_full_page_of_another_subjects_rows() {
    let fixture = MapFixture::create();
    let selected: Vec<String> = (0..2)
        .map(|index| seed_task(&fixture, SELECTED, &format!("sel-task-{index}"), OLDER))
        .collect();
    let other: Vec<String> = (0..=HISTORY_LIMIT)
        .map(|index| seed_task(&fixture, OTHER, &format!("other-task-{index}"), NEWER))
        .collect();

    let payload = selected_payload(&fixture);
    let envelope = envelope(&payload);
    let carried: HashSet<&str> = envelope
        .entities
        .tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .collect();
    for task_id in &selected {
        assert!(
            carried.contains(task_id.as_str()),
            "the payload carries the selected subject's task {task_id}"
        );
    }
    let leaked = other
        .iter()
        .filter(|task_id| carried.contains(task_id.as_str()))
        .count();
    assert_eq!(leaked, 0, "no other subject's task enters the payload");
    assert!(
        payload
            .manifest
            .omissions
            .iter()
            .all(|omission| !omission.contains("task history truncated")),
        "the subject's own task rows stay under the cap: {:?}",
        payload.manifest.omissions
    );
}

/// The failure read's cap counts the selected subject's own rows, so another
/// subject's newer rows cannot push the selected subject's failures out.
#[test]
fn every_selected_subject_failure_survives_a_full_page_of_another_subjects_rows() {
    let fixture = MapFixture::create();
    let selected: Vec<String> = (0..2)
        .map(|index| seed_failure(&fixture, SELECTED, &format!("sel-fail-{index}"), OLDER))
        .collect();
    let other: Vec<String> = (0..=HISTORY_LIMIT)
        .map(|index| seed_failure(&fixture, OTHER, &format!("other-fail-{index}"), NEWER))
        .collect();

    let payload = selected_payload(&fixture);
    let envelope = envelope(&payload);
    let carried: HashSet<&str> = envelope
        .entities
        .failures
        .iter()
        .map(|failure| failure.id.as_str())
        .collect();
    for failure_id in &selected {
        assert!(
            carried.contains(failure_id.as_str()),
            "the payload carries the selected subject's failure {failure_id}"
        );
    }
    let leaked = other
        .iter()
        .filter(|failure_id| carried.contains(failure_id.as_str()))
        .count();
    assert_eq!(leaked, 0, "no other subject's failure enters the payload");
    assert!(
        payload
            .manifest
            .omissions
            .iter()
            .all(|omission| !omission.contains("failure history truncated")),
        "the subject's own failure rows stay under the cap: {:?}",
        payload.manifest.omissions
    );
}
