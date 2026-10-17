// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the export destination custody store on Postgres: a
//! pending save and read-back after a reopen, refusal of a duplicate action
//! id, the delivered and failed marks, the pending-only open read, and the
//! not-found mark.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - The Postgres save/mark/read/refusal assertions
//!
//! ## Does not own
//! - The store implementation (`destination_custody`)
//! - The slot DDL text (`migrate`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`destination_custody_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the save/mark rules or the read-back shape changes.
//! The delivery orchestration belongs to the consumer's own suite.
//!
//! Test-only. Never compiled into production builds.

use crate::destination_custody::{DestinationCustodyRow, DestinationCustodyStore};
use crate::erase_test_support::local_kms;
use crate::postgres::PostgresStore;
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
        updated_at: "2026-02-01T00:00:00Z".to_owned(),
    }
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// The shared Postgres URL, or `None` after noting why the suite skips (the
/// `copy_inventory_pg_tests` fixture shape: the note names the suite and the
/// server's reason, never the connection string).
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres export destination tests", &err);
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic (the `copy_inventory_pg_tests` fixture shape).
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("clio_exportdest_{label}_{}_{nanos}", std::process::id());
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
            crate::pg_skip::unreachable_pg_or_skip("postgres export destination tests", &err);
            None
        }
    }
}

/// A Postgres pending row saves and reads back identically after a reopen.
#[test]
fn postgres_a_pending_row_saves_and_reads_back_after_a_reopen() {
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
        want(store.save_pending(&row("act-1")), "save pending");
    }
    let Some(reopened) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    assert_eq!(
        reopened.load("act-1").expect("load"),
        Some(row("act-1")),
        "the row restarts verbatim"
    );
}

/// A duplicate pending save for the same action id is refused on Postgres too.
#[test]
fn postgres_a_duplicate_pending_save_is_refused() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "duplicate");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
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

/// The delivered and failed marks are visible on Postgres.
#[test]
fn postgres_mark_delivered_and_failed_are_visible() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "marks");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save_pending(&row("act-delivered")), "save delivered");
    want(store.save_pending(&row("act-failed")), "save failed");
    want(
        store.mark_delivered("act-delivered", "2026-02-02T00:00:00Z"),
        "deliver",
    );
    want(
        store.mark_failed("act-failed", "write failed", "2026-02-02T00:00:00Z"),
        "fail",
    );
    let delivered = store.load("act-delivered").expect("load").expect("row");
    assert_eq!(delivered.state, "delivered");
    assert_eq!(delivered.exception_reason, None);
    let failed = store.load("act-failed").expect("load").expect("row");
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.exception_reason.as_deref(), Some("write failed"));
}

/// The open read returns only pending rows on Postgres.
#[test]
fn postgres_open_rows_returns_only_pending() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "open");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save_pending(&row("act-delivered")), "save delivered");
    want(store.save_pending(&row("act-open")), "save open");
    want(
        store.mark_delivered("act-delivered", "2026-02-02T00:00:00Z"),
        "deliver",
    );
    assert_eq!(store.open_rows().expect("open rows"), vec![row("act-open")]);
}

/// Marking an action id with no row is a not-found refusal on Postgres too.
#[test]
fn postgres_marking_an_unknown_action_is_not_found() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "notfound");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    let err = store
        .mark_failed("absent", "reason", "2026-02-02T00:00:00Z")
        .expect_err("an unknown action must be not found");
    assert_eq!(err.code, ErrorCode::NotFound);
}
