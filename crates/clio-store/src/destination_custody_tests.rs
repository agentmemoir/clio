// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the export destination custody store on SQLite: a pending
//! save and read-back, the copy-custody digest and receiver fields and their
//! blank-field refusal, refusal of a duplicate action id, the delivered and
//! failed marks, the pending-only open read, the ordered list read, the
//! not-found mark, and the blank-field refusal.
//!
//! ## Owns
//! - The SQLite save/mark/read/refusal assertions
//!
//! ## Does not own
//! - The store implementation (`destination_custody`)
//! - The slot DDL text (`migrate`)
//! - The Postgres backend assertions (`destination_custody_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the save/mark rules or the read-back shape changes.
//! The delivery orchestration belongs to the consumer's own suite.
//!
//! Test-only. Never compiled into production builds.

use crate::destination_custody::{DestinationCustodyRow, DestinationCustodyStore};
use crate::erase_test_support::local_kms;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};

/// A pending row with fixed field values; override one field per test.
fn row(action_id: &str) -> DestinationCustodyRow {
    DestinationCustodyRow {
        action_id: action_id.to_owned(),
        destination_path: "/tmp/export-dest.bin".to_owned(),
        custody_owner: "clio".to_owned(),
        required_action: "delivery".to_owned(),
        state: "pending".to_owned(),
        exception_reason: None,
        copy_digest: None,
        receiver: None,
        updated_at: "2026-02-01T00:00:00Z".to_owned(),
    }
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(local_kms()).expect("in-memory sqlite opens")
}

/// A new pending row saves and reads back with every field it was given.
#[test]
fn a_pending_row_saves_and_reads_back_with_its_fields() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-1")), "save pending");
    let loaded = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(loaded, row("act-1"));
}

/// A pending row carrying the copy-custody digest and receiver saves and
/// reads both back.
#[test]
fn a_pending_row_saves_and_reads_back_its_copy_custody_fields() {
    let store = open_sqlite();
    let mut pending = row("act-1");
    pending.copy_digest = Some("sha256:abc".to_owned());
    pending.receiver = Some("receiver-1".to_owned());
    want(store.save_pending(&pending), "save pending");
    assert_eq!(
        store.load("act-1").expect("load").expect("row exists"),
        pending,
        "the copy-custody fields restart verbatim"
    );
}

/// A blank copy-custody field is refused before any write.
#[test]
fn a_blank_copy_custody_field_is_refused() {
    let store = open_sqlite();
    let mut bad_digest = row("act-1");
    bad_digest.copy_digest = Some("  ".to_owned());
    let err = store
        .save_pending(&bad_digest)
        .expect_err("a blank copy digest must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let mut bad_receiver = row("act-1");
    bad_receiver.receiver = Some("  ".to_owned());
    let err = store
        .save_pending(&bad_receiver)
        .expect_err("a blank receiver must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("act-1").expect("load"),
        None,
        "nothing was written"
    );
}

/// A duplicate pending save for the same action id is refused and changes
/// nothing: the stored row keeps its original values.
#[test]
fn a_duplicate_pending_save_is_refused() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-1")), "first save");
    let err = store
        .save_pending(&row("act-1"))
        .expect_err("a duplicate action id must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("act-1").expect("load"),
        Some(row("act-1")),
        "the stored row is untouched"
    );
}

/// Marking a row delivered makes the new state and update time visible.
#[test]
fn mark_delivered_is_visible() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-1")), "save pending");
    want(
        store.mark_delivered("act-1", "2026-02-02T00:00:00Z"),
        "mark delivered",
    );
    let loaded = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(loaded.state, "delivered");
    assert_eq!(loaded.exception_reason, None);
    assert_eq!(loaded.updated_at, "2026-02-02T00:00:00Z");
}

/// Marking a row failed makes the failed state and its reason visible.
#[test]
fn mark_failed_is_visible_with_the_reason() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-1")), "save pending");
    want(
        store.mark_failed(
            "act-1",
            "destination already exists",
            "2026-02-02T00:00:00Z",
        ),
        "mark failed",
    );
    let loaded = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(loaded.state, "failed");
    assert_eq!(
        loaded.exception_reason.as_deref(),
        Some("destination already exists")
    );
    assert_eq!(loaded.updated_at, "2026-02-02T00:00:00Z");
}

/// The open read returns only pending rows; a delivered or failed row leaves
/// the reconciliation queue.
#[test]
fn open_rows_returns_only_pending() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-delivered")), "save delivered");
    want(store.save_pending(&row("act-failed")), "save failed");
    want(store.save_pending(&row("act-open")), "save open");
    want(
        store.mark_delivered("act-delivered", "2026-02-02T00:00:00Z"),
        "deliver",
    );
    want(
        store.mark_failed("act-failed", "write failed", "2026-02-02T00:00:00Z"),
        "fail",
    );
    let open = store.open_rows().expect("open rows");
    assert_eq!(open, vec![row("act-open")]);
}

/// Every stored row comes back from `load_all`, ordered by action id.
#[test]
fn load_all_lists_every_row_in_id_order() {
    let store = open_sqlite();
    want(store.save_pending(&row("act-2")), "save act-2");
    want(store.save_pending(&row("act-1")), "save act-1");
    let all = store.load_all().expect("load all");
    let ids: Vec<_> = all.iter().map(|r| r.action_id.as_str()).collect();
    assert_eq!(ids, vec!["act-1", "act-2"], "the restart read is ordered");
}

/// Marking an action id with no row is a not-found refusal.
#[test]
fn marking_an_unknown_action_is_not_found() {
    let store = open_sqlite();
    let err = store
        .mark_delivered("absent", "2026-02-02T00:00:00Z")
        .expect_err("an unknown action must be not found");
    assert_eq!(err.code, ErrorCode::NotFound);
}

/// A pending save with a blank required field is refused before any write.
#[test]
fn a_blank_required_field_is_refused() {
    let store = open_sqlite();
    let mut bad = row("act-1");
    bad.destination_path = "  ".to_owned();
    let err = store
        .save_pending(&bad)
        .expect_err("a blank destination path must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("act-1").expect("load"),
        None,
        "nothing was written"
    );
}

// ---------------------------------------------------------------------------
// Postgres (see `destination_custody_pg_tests`)
// ---------------------------------------------------------------------------
