// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the copy inventory store on SQLite: save and read-back
//! with every metadata field, no-op retries, same-request updates, refusal
//! of a reused copy id bound to a different request or custody, refusal of
//! blank fields, convergence of a pre-existing single-key placeholder table
//! and of a full-shape table stranded without its `copy_id` unique guarantee,
//! verbatim persistence across a reopen, and one-winner races on one id.
//!
//! ## Owns
//! - The SQLite save/read-back/refusal/convergence/race assertions
//! - The scratch SQLite directory fixture the file-backed reopen test uses
//!
//! ## Does not own
//! - The store implementation (`copy_inventory`)
//! - The slot DDL text (`migrate`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The Postgres backend assertions (`copy_inventory_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the save/reuse rules, the read-back shape, or the
//! restart behavior changes. The custody classification rules and the
//! copy-result vocabulary belong to the consumer's own suites.
//!
//! Test-only. Never compiled into production builds.

use crate::copy_inventory::{CopyInventoryRow, CopyInventoryStore};
use crate::durable_requests::DurableRequestRow;
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};

/// A row with fixed field values; override one field per test as needed.
fn row(copy_id: &str, request_id: &str) -> CopyInventoryRow {
    CopyInventoryRow {
        copy_id: copy_id.to_owned(),
        request_id: request_id.to_owned(),
        custody: "subject_local".to_owned(),
        owner: "clio".to_owned(),
        required_action: "confirm_deletion".to_owned(),
        state: "open_gap".to_owned(),
        exception_reason: None,
        updated_at: "2026-02-01T00:00:00Z".to_owned(),
    }
}

/// An open-gap row: the copy is attached to its request with a named
/// exception reason, as a restart must find it.
fn saved_row() -> CopyInventoryRow {
    let mut saved = row("copy-1", "req-1");
    saved.exception_reason = Some("outcome write lost to a restart".to_owned());
    saved
}

/// A closed row: the request result landed, so no operator action remains.
fn closed_row() -> CopyInventoryRow {
    let mut closed = row("copy-1", "req-1");
    closed.state = "deleted".to_owned();
    closed.required_action = "none".to_owned();
    closed.exception_reason = None;
    closed.updated_at = "2026-02-02T00:00:00Z".to_owned();
    closed
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(local_kms()).expect("in-memory sqlite opens")
}

// ---------------------------------------------------------------------------
// SQLite (in-memory)
// ---------------------------------------------------------------------------

/// A new row saves and reads back with every metadata field it was given.
#[test]
fn a_new_record_saves_and_reads_back_with_its_fields() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "save");
    let loaded = store.load("copy-1").expect("load").expect("row exists");
    assert_eq!(loaded.copy_id, "copy-1");
    assert_eq!(loaded.request_id, "req-1");
    assert_eq!(loaded.custody, "subject_local");
    assert_eq!(loaded.owner, "clio");
    assert_eq!(loaded.required_action, "confirm_deletion");
    assert_eq!(loaded.state, "open_gap");
    assert_eq!(loaded.exception_reason, None, "no exception was given");
    assert_eq!(loaded.updated_at, "2026-02-01T00:00:00Z");
}

/// Resaving the identical row is a no-op: one row, same values. The count
/// pins that the retry did not double-apply, and the verbatim load pins that
/// the first write really landed (a skipped write would read back empty).
#[test]
fn resaving_the_identical_record_is_a_no_op() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "first save");
    want(store.save(&row("copy-1", "req-1")), "retry save");
    let all = store.load_all().expect("load all");
    assert_eq!(all.len(), 1, "no duplicate row from the retry");
    assert_eq!(all[0], row("copy-1", "req-1"));
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(row("copy-1", "req-1")),
        "the write landed exactly once"
    );
    // A retry of an evolved row is a no-op too.
    want(store.save(&saved_row()), "evolve");
    want(store.save(&saved_row()), "retry the evolved row");
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(saved_row()),
        "the evolved row survives its own retry"
    );
}

/// Resaving with the same request and custody updates the row: the evolved
/// state, action, exception, and update time win, while the request and
/// custody never move.
#[test]
fn resaving_with_the_same_request_updates_the_row() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "first save");
    want(store.save(&closed_row()), "second save");
    let loaded = store.load("copy-1").expect("load").expect("row exists");
    assert_eq!(loaded, closed_row(), "the evolved row wins");
}

/// A reused copy id bound to a different request is refused and changes
/// nothing: the copy stays attached to the request that inventoried it.
#[test]
fn a_reused_copy_id_with_a_different_request_is_refused() {
    let store = open_sqlite();
    want(store.save(&saved_row()), "first save");
    let err = store
        .save(&row("copy-1", "req-2"))
        .expect_err("a reused id with a different request must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// A reused copy id with the same request but a different custody is refused
/// and changes nothing: the custody is frozen at first save, like the request.
#[test]
fn a_reused_copy_id_with_a_different_custody_is_refused() {
    let store = open_sqlite();
    want(store.save(&saved_row()), "first save");
    let mut changed = saved_row();
    changed.custody = "caller_supplied".to_owned();
    let Err(err) = store.save(&changed) else {
        panic!("a reused id with a different custody must refuse")
    };
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// A save with any blank required field is refused before any write: one row
/// per field, so a new required field must add its row here, not slip past.
#[test]
fn a_save_with_any_blank_required_field_is_refused() {
    let cases: [(&str, fn(&mut CopyInventoryRow)); 8] = [
        ("copy_id", |row| row.copy_id = "  ".to_owned()),
        ("request_id", |row| row.request_id = String::new()),
        ("custody", |row| row.custody = " ".to_owned()),
        ("owner", |row| row.owner = "\t".to_owned()),
        ("required_action", |row| row.required_action.clear()),
        ("state", |row| row.state = " ".to_owned()),
        ("updated_at", |row| row.updated_at.clear()),
        ("exception_reason", |row| {
            row.exception_reason = Some("  ".to_owned());
        }),
    ];
    for (field, blank) in cases {
        let store = open_sqlite();
        let mut bad = row("copy-1", "req-1");
        blank(&mut bad);
        let Err(err) = store.save(&bad) else {
            panic!("a blank {field} must refuse")
        };
        assert_eq!(err.code, ErrorCode::InvalidArgument, "blank {field}");
        assert_eq!(
            store.load("copy-1").expect("load"),
            None,
            "nothing was written for a blank {field}"
        );
    }
}

/// Every saved row comes back from `load_all`, ordered by copy id.
#[test]
fn load_all_lists_every_saved_copy_in_id_order() {
    let store = open_sqlite();
    want(store.save(&row("copy-2", "req-1")), "save copy-2");
    want(store.save(&row("copy-1", "req-1")), "save copy-1");
    let all = store.load_all().expect("load all");
    let ids: Vec<_> = all.iter().map(|r| r.copy_id.as_str()).collect();
    assert_eq!(ids, vec!["copy-1", "copy-2"], "the restart read is ordered");
}

/// A durable request row with fixed field values, for open-gap tests.
fn request_row(request_id: &str) -> DurableRequestRow {
    DurableRequestRow {
        request_id: request_id.to_owned(),
        kind: "erasure".to_owned(),
        state: "received".to_owned(),
        scope: "subject".to_owned(),
        deadline: None,
        record: "{}".to_owned(),
    }
}

/// A copy row whose request has no durable row is reported as a named open
/// gap instead of being silently dropped by a per-request read.
#[test]
fn a_copy_row_without_a_durable_request_is_an_open_gap() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "save the copy");
    assert_eq!(
        store.open_gap_rows().expect("open gap rows"),
        vec![row("copy-1", "req-1")],
        "the unbound copy is the open gap"
    );
}

/// A copy row bound to a stored durable request is not an open gap.
#[test]
fn a_copy_row_bound_to_a_durable_request_is_not_an_open_gap() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "save the copy");
    want(
        crate::durable_requests::DurableRequestStore::save(&store, &request_row("req-1")),
        "save the request",
    );
    assert!(
        store.open_gap_rows().expect("open gap rows").is_empty(),
        "the bound copy is not an open gap"
    );
}

/// Scratch directory removed when the guard drops (the
/// `durable_requests_tests` fixture shape).
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir("clio-store-copy-inventory"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stored row survives a reopen exactly as saved: an open-gap row with a
/// named exception restarts verbatim, with nothing auto-deleted.
#[test]
fn a_saved_record_survives_a_reopen_unchanged() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let store = SqliteStore::open(&path, local_kms()).expect("open");
        want(store.save(&saved_row()), "save");
    }
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    assert_eq!(
        reopened.load("copy-1").expect("load"),
        Some(saved_row()),
        "the row restarts verbatim"
    );
    // A retried write after the restart is a no-op, not a duplicate.
    want(reopened.save(&saved_row()), "retry after reopen");
    assert_eq!(
        reopened.load_all().expect("load all"),
        vec![saved_row()],
        "the retry changed nothing and added no row"
    );
}

/// Two writers racing on one id leave exactly one winner: the store mutex
/// plus the immediate write transaction serialize the saves, so the loser
/// always sees the winner's committed row and is refused, whichever wins.
#[test]
fn two_writers_racing_on_one_id_leave_one_winner() {
    use std::sync::Arc;
    let store = Arc::new(open_sqlite());
    let attached = row("copy-race", "req-1");
    let detached = row("copy-race", "req-2");
    let run = |row: CopyInventoryRow, store: Arc<SqliteStore>| {
        std::thread::spawn(move || store.save(&row))
    };
    let attached_handle = run(attached.clone(), Arc::clone(&store));
    let detached_handle = run(detached.clone(), Arc::clone(&store));
    let attached_outcome = attached_handle.join().expect("attached writer thread");
    let detached_outcome = detached_handle.join().expect("detached writer thread");
    let (winner, loser) = match (attached_outcome, detached_outcome) {
        (Ok(()), Err(loser)) => (&attached, loser),
        (Err(loser), Ok(())) => (&detached, loser),
        _ => panic!("exactly one writer wins"),
    };
    assert_eq!(loser.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("copy-race").expect("load"),
        Some(winner.clone()),
        "the stored row is the winner's"
    );
}

/// A fresh table gains no convergence-only index: `copy_id` is already the
/// primary key, so the save path runs no DDL and the conflict target stays
/// covered.
#[test]
fn a_fresh_table_gains_no_convergence_index() {
    let store = open_sqlite();
    want(store.save(&row("copy-1", "req-1")), "save");
    let indexes: i64 = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'copy_inventory_copy_id_uidx'",
                [],
                |row| row.get(0),
            )
            .am("copy inventory index read")
        })
        .expect("read the copy inventory index");
    assert_eq!(indexes, 0, "a fresh table gained the convergence index");
}

/// A database created under the single-key placeholder shape still saves and
/// loads: the first use converges the stale table onto the metadata columns.
#[test]
fn a_pre_existing_placeholder_table_converges_on_first_use() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        raw.execute_batch("CREATE TABLE copy_inventory (id text PRIMARY KEY);")
            .expect("the old single-key shape");
    }
    let store = SqliteStore::open(&path, local_kms()).expect("open a pre-existing store");
    want(
        store.save(&row("copy-1", "req-1")),
        "save converges and writes",
    );
    // The converged unique index backs the conflict target: a retry is a
    // no-op, not a duplicate row.
    want(
        store.save(&row("copy-1", "req-1")),
        "retry on converged table",
    );
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(row("copy-1", "req-1")),
        "the converged table reads back"
    );
    // A reopen stays converged: the second save is a plain no-op retry.
    drop(store);
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    want(reopened.save(&row("copy-1", "req-1")), "retry after reopen");
    assert_eq!(
        reopened.load_all().expect("load all"),
        vec![row("copy-1", "req-1")],
        "one converged row, unchanged"
    );
}

/// The stored count of indexes carrying the canonical `copy_id` index name.
fn copy_id_index_count(store: &SqliteStore) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'copy_inventory_copy_id_uidx'",
                [],
                |row| row.get(0),
            )
            .am("copy inventory index read")
        })
        .expect("read the copy inventory index")
}

/// A full-shape table with no unique guarantee on `copy_id` — the state an
/// interrupted convergence leaves behind — is healed on the next use: the
/// canonical index appears and the save whose conflict target missed lands.
#[test]
fn a_full_shape_table_without_the_copy_id_guarantee_is_healed() {
    let store = open_sqlite();
    store
        .with_conn(|conn| {
            conn.execute_batch(
                "DROP TABLE copy_inventory; CREATE TABLE copy_inventory (copy_id text,
                 request_id text, custody text, owner text, required_action text, state text,
                 exception_reason text, updated_at text)",
            )
            .am("strand the copy inventory table")
        })
        .expect("strand the table");
    want(store.save(&saved_row()), "save heals the missing guarantee");
    assert_eq!(copy_id_index_count(&store), 1, "canonical index created");
}

/// A partial unique index on `copy_id` backs no `ON CONFLICT (copy_id)`
/// target, so a full-shape table carrying only one is healed too.
#[test]
fn a_partial_unique_index_does_not_back_the_copy_id_conflict_target() {
    let store = open_sqlite();
    store
        .with_conn(|conn| {
            conn.execute_batch(
                "DROP TABLE copy_inventory; CREATE TABLE copy_inventory (copy_id text,
                 request_id text, custody text, owner text, required_action text, state text,
                 exception_reason text, updated_at text);
                 CREATE UNIQUE INDEX copy_inventory_partial_uidx
                     ON copy_inventory (copy_id) WHERE state IS NOT NULL;",
            )
            .am("strand the copy inventory table behind a partial index")
        })
        .expect("strand the table");
    want(store.save(&row("copy-1", "req-1")), "save heals past it");
    assert_eq!(copy_id_index_count(&store), 1, "canonical index created");
}

// ---------------------------------------------------------------------------
// Postgres (see `copy_inventory_pg_tests`)
// ---------------------------------------------------------------------------
