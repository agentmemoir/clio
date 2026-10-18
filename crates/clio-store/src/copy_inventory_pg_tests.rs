// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the copy inventory store on Postgres: save and
//! read-back after a reopen, refusal of a reused copy id bound to a
//! different request or custody, convergence of a stranded full-shape table,
//! and one-winner races on one id.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - The Postgres save/reopen/refusal/convergence/race assertions
//!
//! ## Does not own
//! - The store implementation (`copy_inventory`)
//! - The slot DDL text (`migrate`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`copy_inventory_tests`)
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
use crate::postgres::PostgresStore;
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

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// The shared Postgres URL, or `None` after noting why the suite skips (the
/// `durable_requests_pg_tests` fixture shape: the note names the suite and
/// the server's reason, never the connection string).
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres copy inventory tests", &err);
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic (the `durable_requests_pg_tests` fixture shape).
/// A fresh database guarantees the real copy_inventory columns.
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("clio_copyinv_{label}_{}_{nanos}", std::process::id());
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name}"))
            .expect("drop a leftover scratch database");
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .expect("create a scratch database");
        Self { name }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        if let Ok(mut admin) = crate::pg_tls::connect(&crate::db_url::postgres_url_from_env()) {
            let _ = admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name));
        }
    }
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    crate::db_url::with_database(&crate::db_url::postgres_url_from_env(), name)
}

fn open_pg(url: &str) -> Option<PostgresStore> {
    match PostgresStore::open(url, local_kms()) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres copy inventory tests", &err);
            None
        }
    }
}

/// A Postgres row saves and reads back identically after a reopen.
#[test]
fn postgres_a_record_saves_and_reads_back_after_a_reopen() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "roundtrip");
    drop(admin);
    {
        let Some(store) = open_pg(&url_for_database(&db.name)) else {
            return;
        };
        want(store.save(&saved_row()), "save");
    }
    let Some(reopened) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    assert_eq!(
        reopened.load("copy-1").expect("load"),
        Some(saved_row()),
        "the row restarts verbatim"
    );
    // A retried write after the reopen is a no-op, not a duplicate.
    want(reopened.save(&saved_row()), "retry after reopen");
    assert_eq!(
        reopened.load_all().expect("load all").len(),
        1,
        "the retry added no row"
    );
}

/// A reused copy id bound to a different request or custody is refused on
/// Postgres too.
#[test]
fn postgres_a_reused_copy_id_with_a_different_request_or_custody_is_refused() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "identity");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save(&saved_row()), "first save");
    let err = store
        .save(&row("copy-1", "req-2"))
        .expect_err("a reused id with a different request must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // The custody is frozen too: the same request with a different custody
    // refuses.
    let mut changed = saved_row();
    changed.custody = "caller_supplied".to_owned();
    let err = store
        .save(&changed)
        .expect_err("a reused id with a different custody must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// Two Postgres writers racing on one id leave exactly one winner: the
/// store mutex serializes the saves, so the loser always sees the winner's
/// committed row and is refused, whichever wins.
#[test]
fn postgres_two_writers_racing_on_one_id_leave_one_winner() {
    use std::sync::Arc;
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "race");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    let store = Arc::new(store);
    let attached = row("copy-race", "req-1");
    let detached = row("copy-race", "req-2");
    let run = |row: CopyInventoryRow, store: Arc<PostgresStore>| {
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

/// An upgraded database still saves and loads: a table created under the old
/// single-key placeholder shape is repaired on first use, and the save that
/// the legacy `id` key would have refused lands.
#[test]
fn postgres_a_pre_existing_placeholder_table_converges_on_first_use() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "legacy");
    drop(admin);
    let db_url = url_for_database(&db.name);
    // The old slot file declared `copy_inventory (id text PRIMARY KEY)`, a
    // NOT NULL key on Postgres, so no save could land until the repair.
    let mut legacy = crate::pg_tls::connect(&db_url).expect("legacy connect");
    legacy
        .batch_execute("CREATE TABLE copy_inventory (id text PRIMARY KEY)")
        .expect("the old single-key shape");
    drop(legacy);
    let Some(store) = open_pg(&db_url) else {
        return;
    };
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
    let Some(reopened) = open_pg(&db_url) else {
        return;
    };
    want(reopened.save(&row("copy-1", "req-1")), "retry after reopen");
    assert_eq!(
        reopened.load_all().expect("load all"),
        vec![row("copy-1", "req-1")],
        "one converged row, unchanged"
    );
}

/// A pre-existing full-shape table with no unique guarantee on `copy_id` —
/// the state an interrupted repair leaves behind — is healed on first use:
/// the canonical index appears and the save whose conflict target missed
/// lands.
#[test]
fn postgres_a_full_shape_table_without_the_copy_id_guarantee_is_healed() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "stranded");
    drop(admin);
    let db_url = url_for_database(&db.name);
    let mut stranded = crate::pg_tls::connect(&db_url).expect("stranded connect");
    stranded
        .batch_execute(
            "CREATE TABLE copy_inventory (copy_id text, request_id text, custody text,
                 owner text, required_action text, state text, exception_reason text,
                 updated_at text)",
        )
        .expect("a full-shape table with no unique index");
    drop(stranded);
    let Some(store) = open_pg(&db_url) else {
        return;
    };
    want(store.save(&saved_row()), "save heals the missing guarantee");
    let indexes: i64 = store
        .with_client(|client| {
            Ok(client
                .query_one(
                    "SELECT COUNT(*) FROM pg_indexes
                     WHERE schemaname = current_schema()
                       AND tablename = 'copy_inventory'
                       AND indexname = 'copy_inventory_copy_id_uidx'",
                    &[],
                )
                .am("copy inventory index read")?
                .get::<_, i64>(0))
        })
        .expect("read the copy inventory index");
    assert_eq!(indexes, 1, "canonical index created");
}

/// A fresh table gains no convergence-only index: `copy_id` is already the
/// primary key, so the shape check short-circuits and no DDL runs.
#[test]
fn postgres_a_fresh_table_gains_no_convergence_index() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "freshindex");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save(&saved_row()), "save");
    let indexes: i64 = store
        .with_client(|client| {
            Ok(client
                .query_one(
                    "SELECT COUNT(*) FROM pg_indexes
                     WHERE schemaname = current_schema()
                       AND tablename = 'copy_inventory'
                       AND indexname = 'copy_inventory_copy_id_uidx'",
                    &[],
                )
                .am("copy inventory index read")?
                .get::<_, i64>(0))
        })
        .expect("read the copy inventory index");
    assert_eq!(indexes, 0, "a fresh table gained the convergence index");
}

/// A copy row whose request has no durable row is reported as a named open
/// gap instead of being silently dropped by a per-request read.
#[test]
fn postgres_a_copy_row_without_a_durable_request_is_an_open_gap() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "opengap");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save(&row("copy-1", "req-1")), "save the copy");
    assert_eq!(
        store.open_gap_rows().expect("open gap rows"),
        vec![row("copy-1", "req-1")],
        "the unbound copy is the open gap"
    );
}

/// A copy row bound to a stored durable request is not an open gap.
#[test]
fn postgres_a_copy_row_bound_to_a_durable_request_is_not_an_open_gap() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "boundgap");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
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
