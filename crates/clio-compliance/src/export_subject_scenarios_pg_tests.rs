// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the subject-export
//! scenario tests: a delivered subject export under durable custody, the
//! intent-failure refusal, restart reconciliation of open destinations, and the
//! overwrite refusal, all over a real scoped Postgres store.
//!
//! ## Owns
//! - The Postgres subject-export scenarios, reusing the SQLite slice's driver
//!   and scratch fixtures
//!
//! ## Does not own
//! - The export entrypoint and the delivery sequence (other modules)
//! - The Postgres fixture (`inventory_pg_support_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::export_custody::reconcile_open_destinations;
use crate::export_subject_scenarios_tests::{
    FaultEvents, NOW, SubjectExport, accepted_controls, harness_ctx,
};
use crate::export_tests::{Scratch, seed_pending, staging_entries};
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::OTHER;
use crate::inventory_pg_support_tests::{
    PgLedgerFixture, open_pg_ledger_fixture, pg_gate_or_panic,
};
use crate::test_unique::unique;
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, LocalEventIntent,
    LocalEventState, LocalEventStore, PgTestGuard, PostgresStore,
};
use clio_types::ErrorCode;
use std::sync::Arc;

/// The live fixture, or a printed credential-safe skip note plus `None`.
fn pg_fixture_or_skip() -> Option<(PgTestGuard, PgLedgerFixture)> {
    pg_gate_or_panic(
        "subject export Postgres scenarios",
        open_pg_ledger_fixture(),
    )
}

/// A reopened store handle over the fixture's scoped schema: a fresh handle,
/// not the one that wrote the rows, so a reconcile reads restart state.
fn reopen_scoped_store(fixture: &PgLedgerFixture) -> PostgresStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    PostgresStore::open(fixture.scoped_url(), kms).expect("reopen the scoped Postgres store")
}

#[test]
fn subject_export_pg_delivers_under_custody_with_only_the_selected_subject() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-pg-subj");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = InventoryFacts::Postgres(fixture.target_state());
    let outcome = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Postgres,
        controls: &controls,
        action_id: &action_id,
    }
    .run()
    .expect("the Postgres subject export delivers");

    assert!(scratch.destination().exists(), "the destination exists");
    let row = fixture
        .store
        .load(&action_id)
        .expect("load")
        .expect("custody row");
    assert_eq!(row.state, "delivered");
    assert_eq!(row.custody_owner, "clio");
    assert_eq!(row.required_action, "delivery");
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
    let text = std::fs::read_to_string(scratch.destination()).expect("read the payload");
    assert!(
        !text.contains(OTHER),
        "no other subject marker in the payload"
    );
    let items = outcome
        .manifest
        .counts_by_type
        .iter()
        .find(|(name, _)| name == "items")
        .expect("the manifest carries the items count");
    assert_eq!(
        items.1, 5,
        "the manifest count is the subject mapping's selected count"
    );
}

#[test]
fn subject_export_pg_intent_failure_refuses_with_no_stage_or_destination() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-pg-intent");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = InventoryFacts::Postgres(fixture.target_state());
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Postgres,
        controls: &controls,
        action_id: &action_id,
    }
    .run_with_events(&FaultEvents::intent_fails())
    .expect_err("an intent failure refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!scratch.destination().exists(), "no destination write");
    assert!(!staging.exists(), "no staging directory was created");
    assert!(fixture.store.load(&action_id).expect("load").is_none());
}

#[test]
fn subject_export_pg_outcome_failure_keeps_one_pending_action_and_one_delivered_row() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-pg-outcome");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = InventoryFacts::Postgres(fixture.target_state());
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Postgres,
        controls: &controls,
        action_id: &action_id,
    }
    .run_with_events(&FaultEvents::outcome_fails_over(&fixture.store))
    .expect_err("an outcome failure is reported");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(scratch.destination().exists(), "the file was delivered");
    let row = fixture
        .store
        .load(&action_id)
        .expect("load")
        .expect("custody row");
    assert_eq!(
        row.state, "delivered",
        "the delivered copy is recorded once"
    );
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![action_id.clone()],
        "exactly one pending action remains"
    );

    // A restart reopens the same scoped schema under a fresh handle, and the
    // reconcile completes that action once and never writes a second bundle.
    let before = std::fs::read(scratch.destination()).expect("read the payload");
    let restarted = reopen_scoped_store(&fixture);
    let open = reconcile_open_destinations(&restarted, &restarted).expect("reconcile");
    assert!(
        open.is_empty(),
        "the delivered row is not an open destination"
    );
    assert!(
        restarted.pending_actions().expect("pending").is_empty(),
        "the one pending action is completed exactly once"
    );
    let recorded: Vec<_> = restarted
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    assert_eq!(recorded.len(), 1, "exactly one event row for the action");
    assert_eq!(
        recorded[0].state,
        LocalEventState::Allowed,
        "the delivered action is recorded allowed"
    );
    assert_eq!(
        std::fs::read(scratch.destination()).expect("read the payload"),
        before,
        "reconciliation writes no second bundle"
    );
}

#[test]
fn subject_export_pg_reconcile_reports_open_destinations_without_writing() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let absent = scratch.destination();
    let absent_id = unique("crash-pg-stage");
    seed_pending(&fixture.store, &fixture.store, &absent_id, &absent);
    let landed = scratch.destination().with_file_name("landed.bin");
    std::fs::write(&landed, b"landed-bytes").expect("write the landed copy");
    let landed_id = unique("crash-pg-landed");
    seed_pending(&fixture.store, &fixture.store, &landed_id, &landed);

    // The reconcile runs over a fresh handle: the rows it reports were read
    // back as restart state, not through the handle that wrote them.
    let restarted = reopen_scoped_store(&fixture);
    let open = reconcile_open_destinations(&restarted, &restarted).expect("reconcile");
    let reported: Vec<(&str, bool)> = open
        .iter()
        .map(|row| (row.action_id.as_str(), row.destination_present))
        .collect();
    assert_eq!(reported.len(), 2, "both open rows are reported");
    assert!(
        reported.contains(&(absent_id.as_str(), false)),
        "the absent destination is a known open copy: {reported:?}"
    );
    assert!(
        reported.contains(&(landed_id.as_str(), true)),
        "the landed destination is reported present: {reported:?}"
    );
    assert!(!absent.exists(), "reconciliation writes no destination");
    assert_eq!(
        std::fs::read(&landed).expect("read the landed copy"),
        b"landed-bytes",
        "reconciliation never overwrites or redelivers"
    );
    let pending = restarted.pending_actions().expect("pending");
    assert!(
        !pending.contains(&absent_id) && !pending.contains(&landed_id),
        "each open action's outcome was completed: {pending:?}"
    );
}

#[test]
fn subject_export_pg_reconcile_completes_a_delivered_action_exactly_once() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let destination = scratch.destination();
    std::fs::write(&destination, b"delivered-bytes").expect("write the delivered copy");
    let action_id = unique("exp-pg-delivered");
    fixture
        .store
        .record_intent(&LocalEventIntent {
            action_id: &action_id,
            actor: "tester",
            scope: "export_custody",
            kind: "export_delivery",
            event_time: NOW,
        })
        .expect("record intent");
    fixture
        .store
        .save_pending(&DestinationCustodyRow {
            action_id: action_id.clone(),
            destination_path: destination.display().to_string(),
            custody_owner: "clio".to_owned(),
            required_action: "delivery".to_owned(),
            state: "pending".to_owned(),
            exception_reason: None,
            updated_at: NOW.to_owned(),
        })
        .expect("save pending");
    // The delivery landed and the row was marked delivered, but the outcome
    // write never completed: a restart reconcile must finish it.
    fixture
        .store
        .mark_delivered(&action_id, NOW)
        .expect("mark delivered");
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![action_id.clone()],
        "exactly one action waits for an outcome"
    );

    let before = std::fs::read(&destination).expect("read the payload");
    let restarted = reopen_scoped_store(&fixture);
    let open = reconcile_open_destinations(&restarted, &restarted).expect("reconcile");
    assert!(
        open.is_empty(),
        "the delivered row is not an open destination"
    );
    assert!(
        restarted.pending_actions().expect("pending").is_empty(),
        "the one action is completed exactly once"
    );
    let recorded: Vec<_> = restarted
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    assert_eq!(recorded.len(), 1, "exactly one event row for the action");
    assert_eq!(
        recorded[0].state,
        LocalEventState::Allowed,
        "the delivered action is recorded allowed"
    );
    assert_eq!(
        std::fs::read(&destination).expect("read the payload"),
        before,
        "reconciliation writes no second bundle"
    );
}

#[test]
fn subject_export_pg_refuses_an_existing_destination_and_leaves_outside_files() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    std::fs::create_dir_all(&staging).expect("create staging");
    let foreign = staging.join("keep.bin");
    std::fs::write(&foreign, b"caller-staging").expect("write the foreign file");
    let dest = scratch.destination();
    std::fs::write(&dest, b"caller-bytes").expect("write the caller file");
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-pg-exists");
    let dest_str = dest.to_string_lossy().into_owned();
    let facts = InventoryFacts::Postgres(fixture.target_state());
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Postgres,
        controls: &controls,
        action_id: &action_id,
    }
    .run()
    .expect_err("an existing destination refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        std::fs::read(&dest).expect("read the caller file"),
        b"caller-bytes",
        "the caller file is never overwritten"
    );
    assert_eq!(
        staging_entries(&staging),
        vec![foreign],
        "only the foreign staging file remains"
    );
    assert_eq!(
        fixture
            .store
            .load(&action_id)
            .expect("load")
            .expect("row")
            .state,
        "failed"
    );
}
