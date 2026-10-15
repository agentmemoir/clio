// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite review-export scenario tests: an
//! ordered content-free review export delivered through the durable event and
//! destination-custody boundary, the intent-failure refusal, and the
//! outcome-failure restart reconciliation.
//!
//! ## Owns
//! - The shared delivery-spec, harness-context, accepted-controls, and
//!   injected-fault event writer reused by the Postgres twin
//! - Every SQLite assertion about a delivered or refused review export
//!
//! ## Does not own
//! - The review-export writer and the delivery sequence (other modules)
//! - The shared scratch fixture (`export_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::export_custody::{DeliverySpec, reconcile_open_destinations};
use crate::export_tests::{Scratch, staging_entries, store};
use crate::local_review::LocalReviewQuery;
use crate::subject_request_events::BindingControls;
use crate::test_unique::unique;
use crate::write_review_export;
use clio_store::{
    DestinationCustodyStore, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    SqliteStore,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use std::path::Path;

/// The time the tests pass to every intent and delivery.
pub(crate) const NOW: &str = "2026-01-01T00:00:00Z";

/// A marker that stands for another subject's content carried on an outcome.
pub(crate) const MARKER: &str = "SECRET-OTHER-SUBJECT-marker-7c1f";

/// A review query over a chosen limit and no cursor.
pub(crate) fn query(limit: usize) -> LocalReviewQuery {
    LocalReviewQuery {
        limit,
        cursor: None,
    }
}

/// A fixed local event intent for one action id.
pub(crate) fn intent(action_id: &str) -> LocalEventIntent<'_> {
    LocalEventIntent {
        action_id,
        actor: "tester",
        scope: "review_scenario",
        kind: "local_probe",
        event_time: NOW,
    }
}

/// A delivery spec over one export action, destination, and staging directory.
pub(crate) fn spec<'a>(
    action_id: &'a str,
    destination: &'a Path,
    staging_dir: &'a Path,
) -> DeliverySpec<'a> {
    DeliverySpec {
        action_id,
        destination,
        staging_dir,
        custody_owner: "clio",
        required_action: "delivery",
        now: NOW,
    }
}

/// A privileged operation context for one bank.
pub(crate) fn harness_ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), Actor::Harness)
}

/// The controls that accept the copy for a gated export.
pub(crate) fn accepted_controls() -> BindingControls {
    BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    }
}

/// An injected event-writer fault over a borrowed store.
pub(crate) struct Fault<'a> {
    inner: &'a dyn LocalEventStore,
    fail_intent: bool,
    fail_outcome: bool,
}

impl<'a> Fault<'a> {
    /// An intent write that always fails before any effect.
    pub(crate) fn intent_fails(inner: &'a dyn LocalEventStore) -> Self {
        Self {
            inner,
            fail_intent: true,
            fail_outcome: false,
        }
    }

    /// A real store that records intents but fails every outcome write.
    pub(crate) fn outcome_fails(inner: &'a dyn LocalEventStore) -> Self {
        Self {
            inner,
            fail_intent: false,
            fail_outcome: true,
        }
    }
}

impl LocalEventStore for Fault<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        if self.fail_intent {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected intent-write failure",
            ));
        }
        self.inner.record_intent(intent)
    }

    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        if self.fail_outcome {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected outcome-write failure",
            ));
        }
        self.inner.record_outcome(action_id, denied, result)
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.inner.pending_actions()
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.inner.list_events()
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.inner.prune_completed_before(cutoff)
    }
}

/// Seed three ordered events; the first carries a marker on its outcome.
fn seed_events(store: &SqliteStore) -> (String, String, String) {
    let first = unique("rev-a");
    let second = unique("rev-b");
    let third = unique("rev-c");
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
    (first, second, third)
}

/// An accepted export writes the ordered content-free rows under custody.
#[test]
fn review_export_delivers_ordered_content_free_rows_under_custody() {
    let store = store();
    let (first, second, third) = seed_events(&store);
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-export");
    let ctx = harness_ctx("review-bank");
    write_review_export(
        &ctx,
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect("the review export delivers");

    let text = std::fs::read_to_string(&destination).expect("read the export");
    assert_eq!(
        text,
        format!("1\t{first}\tallowed\n2\t{second}\tpending\n3\t{third}\tdenied\n")
    );
    assert!(
        !text.contains(MARKER),
        "no other subject content in the copy: {text}"
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

/// An intent failure refuses with no staging directory and no destination.
#[test]
fn review_export_intent_failure_leaves_no_staging_and_no_destination() {
    let store = store();
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-intent");
    let ctx = harness_ctx("review-bank");
    let fault = Fault::intent_fails(&store);
    let err = write_review_export(
        &ctx,
        &fault,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect_err("an intent failure refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!destination.exists(), "no destination write");
    assert!(!staging.exists(), "no staging directory was created");
    assert!(store.load(&export_id).expect("load").is_none());
}

/// A directory at the destination is refused by the review writer and left
/// in place.
#[test]
fn review_export_refuses_a_directory_destination_and_leaves_it_in_place() {
    let store = store();
    seed_events(&store);
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    std::fs::create_dir_all(&destination).expect("create the directory destination");
    let export_id = unique("rev-dir");
    let ctx = harness_ctx("review-bank");
    let err = write_review_export(
        &ctx,
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect_err("a directory destination refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("review file export is refused"),
        "the refusal is named by the review writer: {}",
        err.message
    );
    assert!(destination.is_dir(), "the directory is left in place");
    assert!(
        staging_entries(&staging).is_empty(),
        "no managed staging file is written"
    );
    let row = store.load(&export_id).expect("load").expect("custody row");
    assert_eq!(row.state, "failed");
}

/// An outcome failure after delivery keeps the delivered copy and one pending
/// action; a restart reconcile records one outcome and writes no second copy.
#[test]
fn review_export_outcome_failure_keeps_the_copy_and_reconciles_once() {
    let store = store();
    let seed = unique("rev-seed");
    store
        .record_intent(&intent(&seed))
        .expect("seed the intent");
    store
        .record_outcome(&seed, false, "seed allowed")
        .expect("seed the outcome");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let export_id = unique("rev-outcome");
    let ctx = harness_ctx("review-bank");
    let fault = Fault::outcome_fails(&store);
    let err = write_review_export(
        &ctx,
        &fault,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&export_id, &destination, &staging),
    )
    .expect_err("an outcome failure is reported");

    assert_eq!(err.code, ErrorCode::Internal);
    let before = std::fs::read(&destination).expect("read the delivered copy");
    assert_eq!(before, format!("1\t{seed}\tallowed\n").into_bytes());
    let row = store.load(&export_id).expect("load").expect("custody row");
    assert_eq!(
        row.state, "delivered",
        "the delivered copy is recorded once"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![export_id.clone()],
        "exactly one pending action remains"
    );

    let open = reconcile_open_destinations(&store, &store).expect("reconcile");
    assert!(
        open.is_empty(),
        "the delivered row is not an open destination"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the one pending action is completed exactly once"
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
