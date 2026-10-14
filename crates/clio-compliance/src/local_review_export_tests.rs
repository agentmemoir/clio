// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the internal file-export writer of
//! local event review: the refusal when the copy controls are absent, the
//! ordered content-free export when they are accepted, the write-failure arm,
//! and the actor refusal.
//!
//! ## Owns
//! - The in-memory store fixture, the query helper, the delivery-spec builder,
//!   and the scratch export and staging paths
//!
//! ## Does not own
//! - The file-export writer under test and the review module
//! - The shared scratch fixture (`export_tests.rs`, reused)
//! - The privileged review page (see `local_review_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the export gate, the rendered export shape, or the
//! actor gate changes.
//!
//! Test-only. Never compiled into production builds.

use super::write_review_export;
use crate::export_custody::DeliverySpec;
use crate::export_tests::Scratch;
use crate::local_review::LocalReviewQuery;
use crate::subject_request_events::BindingControls;
use crate::test_unique::unique;
use clio_store::{
    DekProvider, DestinationCustodyStore, LocalDevKms, LocalEventIntent, LocalEventStore,
    SqliteStore,
};
use clio_types::{Actor, Bank, ErrorCode, OpContext};
use std::path::Path;
use std::sync::Arc;

/// Event time the tests pass to every intent and delivery.
const EVENT_TIME: &str = "2026-01-01T00:00:00Z";

/// In-memory SQLite store (the `local_events_tests` fixture shape).
fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// An operation context for one actor.
fn ctx(actor: Actor) -> OpContext {
    OpContext::new(Bank::new("review-bank"), actor)
}

/// A review query with a chosen limit and no cursor.
fn query(limit: usize) -> LocalReviewQuery {
    LocalReviewQuery {
        limit,
        cursor: None,
    }
}

/// An intent with fixed field values.
fn intent(action_id: &str) -> LocalEventIntent<'_> {
    LocalEventIntent {
        action_id,
        actor: "tester",
        scope: "scope-opaque",
        kind: "local_probe",
        event_time: EVENT_TIME,
    }
}

/// Persist one intent, naming the step on failure.
fn want_intent(store: &dyn LocalEventStore, action_id: &str) {
    store
        .record_intent(&intent(action_id))
        .unwrap_or_else(|err| panic!("intent {action_id} failed: {err}"));
}

/// The controls that accept the copy for a gated file export.
fn accepted_controls() -> BindingControls {
    BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    }
}

/// A delivery spec over one export action, destination, and staging directory.
fn spec<'a>(action_id: &'a str, destination: &'a Path, staging_dir: &'a Path) -> DeliverySpec<'a> {
    DeliverySpec {
        action_id,
        destination,
        staging_dir,
        custody_owner: "clio",
        required_action: "delivery",
        now: EVENT_TIME,
    }
}

/// With the default controls the export is refused for every scope and no file
/// appears.
#[test]
fn the_file_export_is_refused_with_default_controls() {
    let store = store();
    want_intent(&store, "act-1");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let err = write_review_export(
        &ctx(Actor::Harness),
        &store,
        &store,
        &query(10),
        &BindingControls::default(),
        &spec(&unique("review-export"), &destination, &staging),
    )
    .expect_err("default controls must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(!destination.exists(), "a refused export creates no file");
}

/// With the copy controls accepted the export writes the ordered rows under
/// durable custody.
#[test]
fn the_file_export_writes_the_ordered_rows_when_accepted() {
    let store = store();
    want_intent(&store, "act-1");
    want_intent(&store, "act-2");
    store
        .record_outcome("act-1", false, "allowed action")
        .expect("outcome");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let action_id = unique("review-export");
    write_review_export(
        &ctx(Actor::Harness),
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&action_id, &destination, &staging),
    )
    .expect("accepted export writes");
    let text = std::fs::read_to_string(&destination).expect("read export");
    assert_eq!(text, "1\tact-1\tallowed\n2\tact-2\tpending\n");
    assert_eq!(
        store
            .load(&action_id)
            .expect("load")
            .expect("custody row")
            .state,
        "delivered"
    );
}

/// The export omits the outcome result text, so no content marker leaks.
#[test]
fn the_file_export_omits_the_outcome_result_text() {
    const MARKER: &str = "SECRET-content-marker-9f3a";
    let store = store();
    want_intent(&store, "act-1");
    store
        .record_outcome("act-1", true, MARKER)
        .expect("outcome");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    write_review_export(
        &ctx(Actor::Harness),
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&unique("review-export"), &destination, &staging),
    )
    .expect("accepted export writes");
    let text = std::fs::read_to_string(&destination).expect("read export");
    assert!(
        !text.contains(MARKER),
        "the export must omit the outcome result text: {text}"
    );
    assert!(text.contains("act-1"));
}

/// A destination whose parent directory is absent fails as an internal error
/// and leaves no destination file.
#[test]
fn a_write_failure_is_an_internal_error() {
    let store = store();
    want_intent(&store, "act-1");
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.root.join("absent").join("export.tsv");
    let err = write_review_export(
        &ctx(Actor::Harness),
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&unique("review-export"), &destination, &staging),
    )
    .expect_err("a missing parent directory must fail the write");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        !destination.exists(),
        "a failed write leaves no destination file"
    );
}

/// An unprivileged actor is refused.
#[test]
fn the_file_export_refuses_an_unprivileged_actor() {
    let store = store();
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    let destination = scratch.destination();
    let err = write_review_export(
        &ctx(Actor::User),
        &store,
        &store,
        &query(10),
        &accepted_controls(),
        &spec(&unique("review-export"), &destination, &staging),
    )
    .expect_err("unprivileged export");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(!destination.exists());
}
