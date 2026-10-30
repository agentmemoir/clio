// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite export-destination custody column convergence:
//! a pre-existing table gains both columns, a partly converged table gains only
//! what it lacks, the convergence is idempotent, and an absent table is a
//! no-op.
//!
//! ## Owns
//! - Test assertions for `converge_sqlite`
//!
//! ## Does not own
//! - Production convergence SQL (`destination_custody_converge`)
//! - The Postgres convergence assertions (`destination_custody_converge_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::destination_custody_converge::converge_sqlite;

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare");
    stmt.query_map([], |r| r.get::<_, String>(1))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
}

/// A legacy table without the copy-custody columns gains both, keeps its rows,
/// and a second pass adds nothing.
#[test]
fn adds_the_copy_custody_columns_and_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE export_destinations (
            action_id text PRIMARY KEY,
            destination_path text NOT NULL,
            custody_owner text NOT NULL,
            required_action text NOT NULL,
            state text NOT NULL,
            exception_reason text,
            updated_at text NOT NULL
         );
         INSERT INTO export_destinations VALUES ('act-1', '/tmp/x', 'clio', 'delivery',
            'pending', NULL, '2026-02-01T00:00:00Z');",
    )
    .unwrap();
    converge_sqlite(&conn).unwrap();
    let cols = columns(&conn, "export_destinations");
    assert!(cols.contains(&"copy_digest".to_owned()), "{cols:?}");
    assert!(cols.contains(&"receiver".to_owned()), "{cols:?}");
    // The pre-existing row reads NULL in both new columns.
    let row: (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT copy_digest, receiver FROM export_destinations WHERE action_id = 'act-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("the pre-existing row");
    assert_eq!(row, (None, None), "a pre-convergence row reads NULL");

    // Second pass must not error or duplicate a column.
    converge_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "export_destinations")
            .iter()
            .filter(|c| *c == "copy_digest" || *c == "receiver")
            .count(),
        2,
        "must not be added twice"
    );
}

/// A table that already holds one copy-custody column gains only the other.
#[test]
fn adds_only_the_missing_column() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE export_destinations (
            action_id text PRIMARY KEY,
            destination_path text NOT NULL,
            custody_owner text NOT NULL,
            required_action text NOT NULL,
            state text NOT NULL,
            exception_reason text,
            copy_digest text,
            updated_at text NOT NULL
         );",
    )
    .unwrap();
    converge_sqlite(&conn).unwrap();
    let cols = columns(&conn, "export_destinations");
    assert!(cols.contains(&"receiver".to_owned()), "{cols:?}");
    assert_eq!(
        cols.iter().filter(|c| *c == "copy_digest").count(),
        1,
        "pre-existing column must not be added twice"
    );
}

/// A database without the table is a no-op: the slot DDL declares the columns
/// itself on a fresh database.
#[test]
fn an_absent_export_destinations_table_is_a_noop() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    converge_sqlite(&conn).unwrap();
    assert!(
        columns(&conn, "export_destinations").is_empty(),
        "no table must be created"
    );
}
