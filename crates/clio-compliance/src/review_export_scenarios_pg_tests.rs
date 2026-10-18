// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the review-export
//! scenario tests: an ordered content-free review export delivered through the
//! durable destination-custody boundary, the intent-failure refusal, and the
//! outcome-failure restart reconciliation, all over a real scoped Postgres
//! store.
//!
//! ## Owns
//! - The Postgres review-export scenarios, reusing the SQLite slice's helpers
//!
//! ## Does not own
//! - The review-export writer and the delivery sequence (other modules)
//! - The Postgres fixture (`inventory_pg_support_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::export_custody::reconcile_open_destinations;
use crate::export_tests::{Scratch, staging_entries};
use crate::inventory_pg_support_tests::{
    PgLedgerFixture, open_pg_ledger_fixture, pg_gate_or_panic,
};
use crate::review_export_scenarios_tests::{
    Fault, MARKER, accepted_controls, harness_ctx, intent, query, spec,
};
use crate::test_unique::unique;
use crate::write_review_export;
use clio_store::{DestinationCustodyStore, LocalEventState, LocalEventStore};
use clio_types::ErrorCode;

/// The live fixture, or a printed credential-safe skip note plus `None`.
fn pg_fixture_or_skip() -> Option<(clio_store::PgTestGuard, PgLedgerFixture)> {
    pg_gate_or_panic("review export Postgres scenarios", open_pg_ledger_fixture())
}

/// An accepted Postgres export writes the ordered content-free rows under
/// custody.
#[test]
fn review_export_pg_delivers_ordered_content_free_rows_under_custody() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let store = &fixture.store;
    let first = unique("rev-pg-a");
    let second = unique("rev-pg-b");
    let third = unique("rev-pg-c");
    store
        .record_intent(&intent(&first))
        .expect("seed the first intent");
    store
        .record_outcome(&first, false, MARKER)
        .expect("seed the first outcome");
    store
        .record_intent(&intent(&second))
        .expect("seed the second intent");
    store
        .record_intent(&intent(&third))
        .expect("seed the third intent");
    store
        .record_outcome(&third, true, "denied reason")
        .expect("seed the third outcome");

    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-pg-export");
    let ctx = harness_ctx(&fixture.bank);
    write_review_export(
        &ctx,
        store,
        store,
        &query(1000),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect("the Postgres review export delivers");

    let text = std::fs::read_to_string(&destination).expect("read the export");
    assert!(
        !text.contains(MARKER),
        "no other subject content in the copy: {text}"
    );
    let selected: Vec<&str> = text
        .lines()
        .filter(|line| {
            line.contains(first.as_str())
                || line.contains(second.as_str())
                || line.contains(third.as_str())
        })
        .collect();
    assert_eq!(
        selected.len(),
        3,
        "all three seeded rows are exported: {text}"
    );
    assert!(
        selected[0].ends_with(&format!("\t{first}\tallowed")),
        "first row: {}",
        selected[0]
    );
    assert!(
        selected[1].ends_with(&format!("\t{second}\tpending")),
        "second row: {}",
        selected[1]
    );
    assert!(
        selected[2].ends_with(&format!("\t{third}\tdenied")),
        "third row: {}",
        selected[2]
    );

    let row = store.load(&export_id).expect("load").expect("custody row");
    assert_eq!(row.state, "delivered");
    assert_eq!(row.custody_owner, "clio");
    assert_eq!(row.required_action, "delivery");
    let tied: Vec<_> = store
        .load_all()
        .expect("all custody rows")
        .into_iter()
        .filter(|row| row.destination_path == destination.display().to_string())
        .collect();
    assert_eq!(tied.len(), 1, "one custody row per destination");
    assert_eq!(
        tied[0].action_id, export_id,
        "the custody row names the export action id"
    );
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
}

/// A Postgres intent failure refuses with no staging and no destination.
#[test]
fn review_export_pg_intent_failure_leaves_no_staging_and_no_destination() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let store = &fixture.store;
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-pg-intent");
    let ctx = harness_ctx(&fixture.bank);
    let fault = Fault::intent_fails(store);
    let err = write_review_export(
        &ctx,
        &fault,
        store,
        &query(1000),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect_err("an intent failure refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!destination.exists(), "no destination write");
    assert!(!staging.exists(), "no staging directory was created");
    assert!(store.load(&export_id).expect("load").is_none());
}

/// A Postgres outcome failure after delivery keeps the delivered copy and one
/// pending action; a restart reconcile records one outcome and writes no second
/// copy.
#[test]
fn review_export_pg_outcome_failure_keeps_the_copy_and_reconciles_once() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let store = &fixture.store;
    let seed = unique("rev-pg-seed");
    store
        .record_intent(&intent(&seed))
        .expect("seed the intent");
    store
        .record_outcome(&seed, false, "seed allowed")
        .expect("seed the outcome");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-pg-outcome");
    let ctx = harness_ctx(&fixture.bank);
    let fault = Fault::outcome_fails(store);
    let err = write_review_export(
        &ctx,
        &fault,
        store,
        &query(1000),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect_err("an outcome failure is reported");

    assert_eq!(err.code, ErrorCode::Internal);
    let before = std::fs::read(&destination).expect("read the delivered copy");
    assert!(
        String::from_utf8_lossy(&before).contains(&seed),
        "the delivered copy holds the review rows"
    );
    let row = store.load(&export_id).expect("load").expect("custody row");
    assert_eq!(
        row.state, "delivered",
        "the delivered copy is recorded once"
    );
    assert!(
        store
            .pending_actions()
            .expect("pending")
            .contains(&export_id),
        "the export action waits for its outcome"
    );

    let open = reconcile_open_destinations(store, store).expect("reconcile");
    assert!(
        open.is_empty(),
        "the delivered row is not an open destination"
    );
    assert!(
        !store
            .pending_actions()
            .expect("pending")
            .contains(&export_id),
        "the pending action is completed exactly once"
    );
    let recorded: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == export_id)
        .collect();
    assert_eq!(recorded.len(), 1, "exactly one event row for the action");
    assert_eq!(recorded[0].state, LocalEventState::Allowed);
    assert_eq!(
        std::fs::read(&destination).expect("read the delivered copy"),
        before,
        "reconciliation writes no second delivery"
    );
}
