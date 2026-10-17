// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable request store on SQLite: save and
//! read-back with every field, no-op retries, same-scope updates, refusal of
//! a reused id bound to a different scope or kind, refusal of blank fields,
//! convergence of a pre-existing single-key placeholder table, verbatim
//! persistence across a reopen, and one-winner races on a single id.
//!
//! ## Owns
//! - The SQLite save/read-back/refusal/race assertions
//! - The scratch SQLite directory fixture the file-backed reopen test uses
//!
//! ## Does not own
//! - The store implementation (`durable_requests`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The Postgres backend assertions (`durable_requests_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the save/reuse rules, the read-back shape, or the
//! restart behavior changes. The request vocabulary and its decoding belong
//! to the consumer's own suites.
//!
//! Test-only. Never compiled into production builds.

use crate::durable_requests::{DurableRequestRow, DurableRequestStore};
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};

/// The canonical scope serialization of a bank-wide request.
const BANK_SCOPE: &str = r#"{"bank":{"bank_id":"b1"}}"#;

/// The canonical scope serialization of a single-subject request.
const SUBJECT_SCOPE: &str = r#"{"subject":{"bank_id":"b1","subject_id":"s1"}}"#;

/// A row with fixed field values; override one field per test as needed.
fn row(request_id: &str, scope: &str) -> DurableRequestRow {
    DurableRequestRow {
        request_id: request_id.to_owned(),
        kind: "erasure".to_owned(),
        state: "received".to_owned(),
        scope: scope.to_owned(),
        deadline: None,
        record: format!(r#"{{"request_id":"{request_id}","scope":{scope}}}"#),
    }
}

/// An in-progress row: an operator deadline and a per-path saved attempt and
/// action id inside the record payload, as a restart must find them.
fn saved_row() -> DurableRequestRow {
    let mut saved = row("req-1", SUBJECT_SCOPE);
    saved.state = "in_progress".to_owned();
    saved.deadline = Some("2026-02-01T00:00:00Z".to_owned());
    saved.record =
        r#"{"paths":{"p1":{"owner":"clio","state":{"saved":{"attempt_id":"a1","action_id":"act-1"}}}}}"#
            .to_owned();
    saved
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

/// A new record saves and reads back with every field it was given.
#[test]
fn a_new_record_saves_and_reads_back_with_its_fields() {
    let store = open_sqlite();
    want(store.save(&row("req-1", BANK_SCOPE)), "save");
    let loaded = store.load("req-1").expect("load").expect("row exists");
    assert_eq!(loaded.request_id, "req-1");
    assert_eq!(loaded.kind, "erasure");
    assert_eq!(loaded.state, "received");
    assert_eq!(loaded.scope, BANK_SCOPE);
    assert_eq!(loaded.deadline, None, "no deadline was given");
    assert_eq!(loaded.record, row("req-1", BANK_SCOPE).record);
}

/// Resaving the identical row is a no-op: one row, same values. The count
/// pins that the retry did not double-apply, and the verbatim load pins that
/// the first write really landed (a skipped write would read back empty).
#[test]
fn resaving_the_identical_record_is_a_no_op() {
    let store = open_sqlite();
    want(store.save(&row("req-1", BANK_SCOPE)), "first save");
    want(store.save(&row("req-1", BANK_SCOPE)), "retry save");
    let all = store.load_all().expect("load all");
    assert_eq!(all.len(), 1, "no duplicate row from the retry");
    assert_eq!(all[0], row("req-1", BANK_SCOPE));
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(row("req-1", BANK_SCOPE)),
        "the write landed exactly once"
    );
    // A retry of an evolved row is a no-op too.
    let mut evolved = row("req-1", BANK_SCOPE);
    evolved.state = "in_progress".to_owned();
    want(store.save(&evolved), "evolve");
    want(store.save(&evolved), "retry the evolved row");
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(evolved),
        "the evolved row survives its own retry"
    );
}

/// Resaving with the same scope updates the row: the evolved record, its new
/// state, and its deadline win, while the scope never moves.
#[test]
fn resaving_with_the_same_scope_updates_the_record() {
    let store = open_sqlite();
    want(store.save(&row("req-1", SUBJECT_SCOPE)), "first save");
    want(store.save(&saved_row()), "second save");
    let loaded = store.load("req-1").expect("load").expect("row exists");
    assert_eq!(loaded, saved_row(), "the evolved row wins");
}

/// A reused id bound to a different scope is refused and changes nothing.
#[test]
fn a_reused_id_with_a_different_scope_is_refused() {
    let store = open_sqlite();
    want(store.save(&saved_row()), "first save");
    let err = store
        .save(&row("req-1", BANK_SCOPE))
        .expect_err("a reused id with a different scope must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// A reused id with the same scope but a different kind is refused and
/// changes nothing: the kind is frozen at first save, like the scope.
#[test]
fn a_reused_id_with_a_different_kind_is_refused() {
    let store = open_sqlite();
    want(store.save(&saved_row()), "first save");
    let mut changed = saved_row();
    changed.kind = "export".to_owned();
    let Err(err) = store.save(&changed) else {
        panic!("a reused id with a different kind must refuse")
    };
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// A save with any blank required field is refused before any write: one row
/// per field, so a new required field must add its row here, not slip past.
#[test]
fn a_save_with_any_blank_required_field_is_refused() {
    let cases: [(&str, fn(&mut DurableRequestRow)); 6] = [
        ("request_id", |row| row.request_id = "  ".to_owned()),
        ("kind", |row| row.kind.clear()),
        ("state", |row| row.state = " ".to_owned()),
        ("scope", |row| row.scope = "\t".to_owned()),
        ("record", |row| row.record.clear()),
        ("deadline", |row| row.deadline = Some("  ".to_owned())),
    ];
    for (field, blank) in cases {
        let store = open_sqlite();
        let mut bad = row("req-1", BANK_SCOPE);
        blank(&mut bad);
        let Err(err) = store.save(&bad) else {
            panic!("a blank {field} must refuse")
        };
        assert_eq!(err.code, ErrorCode::InvalidArgument, "blank {field}");
        assert_eq!(
            store.load("req-1").expect("load"),
            None,
            "nothing was written for a blank {field}"
        );
    }
}

/// Every saved row comes back from `load_all`, ordered by request id.
#[test]
fn load_all_lists_every_saved_request_in_id_order() {
    let store = open_sqlite();
    want(store.save(&row("req-2", SUBJECT_SCOPE)), "save req-2");
    want(store.save(&row("req-1", BANK_SCOPE)), "save req-1");
    let all = store.load_all().expect("load all");
    let ids: Vec<_> = all.iter().map(|r| r.request_id.as_str()).collect();
    assert_eq!(ids, vec!["req-1", "req-2"], "the restart read is ordered");
}

/// Scratch directory removed when the guard drops (the
/// `migrate_compliance_slots_tests` fixture shape).
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir(
            "clio-store-durable-requests",
        ))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stored record survives a reopen exactly as saved: an in-progress row
/// with a deadline and a saved per-path attempt restarts verbatim, with
/// nothing auto-completed.
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
        reopened.load("req-1").expect("load"),
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
    let bank = row("req-race", BANK_SCOPE);
    let subject = row("req-race", SUBJECT_SCOPE);
    let run = |row: DurableRequestRow, store: Arc<SqliteStore>| {
        std::thread::spawn(move || store.save(&row))
    };
    let bank_handle = run(bank.clone(), Arc::clone(&store));
    let subject_handle = run(subject.clone(), Arc::clone(&store));
    let bank_outcome = bank_handle.join().expect("bank writer thread");
    let subject_outcome = subject_handle.join().expect("subject writer thread");
    let (winner, loser) = match (bank_outcome, subject_outcome) {
        (Ok(()), Err(loser)) => (&bank, loser),
        (Err(loser), Ok(())) => (&subject, loser),
        _ => panic!("exactly one writer wins"),
    };
    assert_eq!(loser.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("req-race").expect("load"),
        Some(winner.clone()),
        "the stored row is the winner's"
    );
}

/// The six request columns a converged table holds, in DDL order.
fn expected_columns() -> Vec<String> {
    ["request_id", "kind", "state", "scope", "deadline", "record"]
        .iter()
        .map(|column| (*column).to_owned())
        .collect()
}

/// The `durable_requests` columns, in the table's own order.
fn columns(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(durable_requests)")
                .am("durable request table_info")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("durable request table_info rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("durable request table_info rows")?;
            Ok(names)
        })
        .expect("read the durable_requests columns")
}

/// The `durable_requests` index names.
fn index_names(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA index_list(durable_requests)")
                .am("durable request index list")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("durable request index list rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("durable request index list rows")?;
            Ok(names)
        })
        .expect("read the durable_requests indexes")
}

/// The row count of `durable_requests`.
fn row_count(store: &SqliteStore) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM durable_requests", [], |row| {
                row.get(0)
            })
            .am("count the durable request rows")
        })
        .expect("count the durable request rows")
}

/// A database created under the single-key placeholder shape still saves and
/// loads: the first use converges the stale table onto the request columns.
#[test]
fn a_pre_existing_placeholder_table_converges_on_first_use() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        raw.execute_batch(
            "CREATE TABLE durable_requests (id text PRIMARY KEY);
             INSERT INTO durable_requests VALUES ('legacy');",
        )
        .expect("the old single-key shape");
    }
    let store = SqliteStore::open(&path, local_kms()).expect("open a pre-existing store");
    want(
        store.save(&row("req-1", BANK_SCOPE)),
        "save converges and writes",
    );
    // The converged unique index backs the conflict target: a retry is a
    // no-op, not a duplicate row.
    want(
        store.save(&row("req-1", BANK_SCOPE)),
        "retry on converged table",
    );
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(row("req-1", BANK_SCOPE)),
        "the converged table reads back"
    );
    // A reopen stays converged: the second save is a plain no-op retry.
    drop(store);
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    want(
        reopened.save(&row("req-1", BANK_SCOPE)),
        "retry after reopen",
    );
    assert_eq!(
        reopened.load_all().expect("load all"),
        vec![row("req-1", BANK_SCOPE)],
        "one converged row, unchanged"
    );
    let repaired = columns(&reopened);
    assert_eq!(
        repaired,
        expected_columns(),
        "the rebuilt table holds exactly the request columns, in DDL order"
    );
    assert!(
        !repaired.iter().any(|name| name == "id"),
        "the legacy key is gone: {repaired:?}"
    );
    assert!(
        index_names(&reopened)
            .iter()
            .any(|name| name == "durable_requests_request_id_uidx"),
        "the converged table backs the request-id index"
    );
    assert_eq!(
        row_count(&reopened),
        1,
        "the row that carries no request is gone and only the saved row is left"
    );
}

/// Only a row whose six request columns are all NULL is removed. A row that
/// already holds a request value is copied, so the repair never deletes one.
#[test]
fn a_placeholder_row_with_a_request_value_is_kept() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        raw.execute_batch(
            "CREATE TABLE durable_requests (id text PRIMARY KEY);
             ALTER TABLE durable_requests ADD COLUMN request_id text;
             INSERT INTO durable_requests (id, request_id) VALUES ('legacy', 'req-kept');",
        )
        .expect("the placeholder rows");
    }
    let store = SqliteStore::open(&path, local_kms()).expect("open a pre-existing store");
    assert_eq!(
        store.load("absent").expect("converge"),
        None,
        "the read converges the table"
    );
    assert_eq!(row_count(&store), 1, "the row with a request value is kept");
    let kept: Option<String> = store
        .with_conn(|conn| {
            conn.query_row("SELECT request_id FROM durable_requests", [], |row| {
                row.get(0)
            })
            .am("read the kept row")
        })
        .expect("read the kept row");
    assert_eq!(
        kept.as_deref(),
        Some("req-kept"),
        "the copied request value round-trips"
    );
    assert_eq!(
        columns(&store),
        expected_columns(),
        "the rebuilt table holds exactly the request columns, in DDL order"
    );
}

// ---------------------------------------------------------------------------
// Postgres (see `durable_requests_pg_tests`)
// ---------------------------------------------------------------------------
