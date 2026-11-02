// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite `sync_device_high_water` convergence: a database
//! whose open predates the table gains it, the convergence is idempotent, the
//! converged shape matches the fresh-database declaration, and a current
//! database converges on a read-only connection with no write.
//!
//! ## Owns
//! - Test assertions for `converge_sqlite`
//!
//! ## Does not own
//! - Production convergence DDL (`sync_schema_converge`)
//! - The Postgres convergence assertions (`sync_schema_converge_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::SQL_CORE;
use crate::sync_schema_converge::converge_sqlite;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// A fresh directory for one test, removed by the caller.
///
/// The process id and the test tag keep two runs from sharing a file.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("clio-sync-converge-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare");
    stmt.query_map([], |r| r.get::<_, String>(1))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
}

/// The columns a fresh database gets from the portable batch, in DDL order.
fn fresh_columns() -> Vec<String> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(SQL_CORE).unwrap();
    columns(&conn, "sync_device_high_water")
}

/// A database stamped before the table existed gains it on the next open, and a
/// second pass adds nothing.
#[test]
fn a_database_without_the_table_gains_it_and_the_step_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(SQL_CORE).unwrap();
    // The shape an open that predates the table left behind.
    conn.execute_batch("DROP TABLE sync_device_high_water;")
        .unwrap();
    assert!(
        columns(&conn, "sync_device_high_water").is_empty(),
        "the table must be absent before the step runs"
    );

    converge_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "sync_device_high_water"),
        fresh_columns(),
        "the converged shape must match the fresh declaration"
    );

    // A second pass must not error or change the shape.
    converge_sqlite(&conn).unwrap();
    assert_eq!(columns(&conn, "sync_device_high_water"), fresh_columns());
}

/// The step runs on every open, so it must be a no-op on a database that already
/// has the table, including one holding rows.
#[test]
fn a_current_database_is_untouched_and_keeps_its_rows() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(SQL_CORE).unwrap();
    conn.execute(
        "INSERT INTO sync_device_high_water
         (bank_id, device_id, entity_kind, transaction_time, updated_at)
         VALUES ('b1', 'dev-a', 'item', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();

    converge_sqlite(&conn).unwrap();

    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_device_high_water", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 1, "the step must not touch existing rows");
}

/// The primary key covers the bank, device and kind, so one device holds one
/// mark per kind and a repeat of the same triple is refused.
#[test]
fn the_primary_key_covers_the_bank_device_and_kind() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    converge_sqlite(&conn).unwrap();
    conn.execute(
        "INSERT INTO sync_device_high_water
         (bank_id, device_id, entity_kind, transaction_time, updated_at)
         VALUES ('b1', 'dev-a', 'item', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO sync_device_high_water
         (bank_id, device_id, entity_kind, transaction_time, updated_at)
         VALUES ('b1', 'dev-a', 'item', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z')",
        [],
    );
    assert!(duplicate.is_err(), "one mark per bank, device and kind");

    // A second kind is a separate row: mixed encodings mean one per-kind
    // comparison is the only sound one.
    conn.execute(
        "INSERT INTO sync_device_high_water
         (bank_id, device_id, entity_kind, transaction_time, updated_at)
         VALUES ('b1', 'dev-a', 'triple', '2026-01-01', '2026-01-01T00:00:00Z')",
        [],
    )
    .expect("a second kind is a separate mark");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_device_high_water", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 2);
}

/// A current database needs no write, so the step succeeds on a read-only
/// connection and leaves every row it finds alone.
///
/// The connection really does refuse writes: creating a table that is absent
/// fails on it. The step passes only because its probe finds the table and skips
/// the DDL.
#[test]
fn a_current_database_converges_on_a_read_only_connection() {
    let dir = temp_dir("readonly");
    let path = dir.join("store.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(SQL_CORE).unwrap();
    conn.execute(
        "INSERT INTO sync_device_high_water
         (bank_id, device_id, entity_kind, transaction_time, updated_at)
         VALUES ('b1', 'dev-a', 'item', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    drop(conn);

    let readonly = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read-only open");
    assert!(
        readonly
            .execute_batch("CREATE TABLE probe_write (x text);")
            .is_err(),
        "the connection must refuse a write, or this test proves nothing"
    );

    converge_sqlite(&readonly).expect("a current table needs no DDL");

    let (stamp, updated) = readonly
        .query_row(
            "SELECT transaction_time, updated_at FROM sync_device_high_water
             WHERE bank_id = 'b1' AND device_id = 'dev-a' AND entity_kind = 'item'",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .expect("the seeded row");
    assert_eq!(
        stamp, "2026-01-01T00:00:00Z",
        "the step must not touch rows"
    );
    assert_eq!(
        updated, "2026-01-01T00:00:00Z",
        "the step must not touch rows"
    );
    let rows: i64 = readonly
        .query_row("SELECT COUNT(*) FROM sync_device_high_water", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 1, "the step must not add rows");
    drop(readonly);

    cleanup(&dir);
}
