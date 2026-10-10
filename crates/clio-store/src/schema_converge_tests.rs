// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite additive-column convergence tests:
//! the `items` lineage upgrade, the sealed domain-record `context_cipher`
//! upgrade, idempotency, and the fresh-database no-op path.
//!
//! ## Owns
//! - Test assertions for `converge_lineage_columns_sqlite` and
//!   `converge_context_columns_sqlite`
//!
//! ## Does not own
//! - Production convergence SQL (`schema_converge`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{
    converge_context_columns_sqlite, converge_distilled_state_sqlite,
    converge_lineage_columns_sqlite,
};

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare");
    stmt.query_map([], |r| r.get::<_, String>(1))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
}

#[test]
fn adds_missing_lineage_columns_and_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE items (id text PRIMARY KEY, bank_id text NOT NULL);")
        .unwrap();
    converge_lineage_columns_sqlite(&conn).unwrap();
    let cols = columns(&conn, "items");
    assert!(cols.contains(&"corrects_item_id".to_owned()), "{cols:?}");
    assert!(cols.contains(&"superseded_by_id".to_owned()), "{cols:?}");
    // Second pass must not error on the now-present columns.
    converge_lineage_columns_sqlite(&conn).unwrap();
    let cols = columns(&conn, "items");
    assert_eq!(
        cols.iter()
            .filter(|c| *c == "corrects_item_id" || *c == "superseded_by_id")
            .count(),
        2,
        "{cols:?}"
    );
}

#[test]
fn adds_only_the_missing_column() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE items (id text PRIMARY KEY, corrects_item_id text);")
        .unwrap();
    converge_lineage_columns_sqlite(&conn).unwrap();
    let cols = columns(&conn, "items");
    assert!(cols.contains(&"superseded_by_id".to_owned()), "{cols:?}");
    assert_eq!(
        cols.iter().filter(|c| *c == "corrects_item_id").count(),
        1,
        "{cols:?}"
    );
}

#[test]
fn absent_items_table_is_a_noop() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    converge_lineage_columns_sqlite(&conn).unwrap();
    let item_cols = columns(&conn, "items");
    assert!(item_cols.is_empty(), "{item_cols:?}");
}

#[test]
fn adds_missing_context_columns_and_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE beliefs (id text PRIMARY KEY);
         CREATE TABLE persona_stable_entries (id text PRIMARY KEY);
         CREATE TABLE persona_preferences (id text PRIMARY KEY);",
    )
    .unwrap();
    converge_context_columns_sqlite(&conn).unwrap();
    for table in ["beliefs", "persona_stable_entries", "persona_preferences"] {
        let cols = columns(&conn, table);
        assert!(
            cols.contains(&"context_cipher".to_owned()),
            "{table}: {cols:?}"
        );
        // Second pass must not error on the now-present column.
        converge_context_columns_sqlite(&conn).unwrap();
        assert_eq!(
            columns(&conn, table)
                .iter()
                .filter(|c| *c == "context_cipher")
                .count(),
            1,
            "{table}"
        );
    }
}

#[test]
fn adds_preference_context_as_of_column_and_is_idempotent() {
    // A pre-existing `persona_preferences` table gains `context_as_of`; the
    // convergence is a no-op on the second pass.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE persona_preferences (id text PRIMARY KEY, context_cipher text);",
    )
    .unwrap();
    converge_context_columns_sqlite(&conn).unwrap();
    assert!(columns(&conn, "persona_preferences").contains(&"context_as_of".to_owned()));
    converge_context_columns_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "persona_preferences")
            .iter()
            .filter(|c| *c == "context_as_of")
            .count(),
        1,
        "must not be added twice"
    );
}

#[test]
fn adds_belief_context_as_of_column_and_is_idempotent() {
    // A pre-existing `beliefs` table gains `context_as_of` alongside the
    // preference column; the convergence is a no-op on the second pass.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE beliefs (id text PRIMARY KEY, context_cipher text);
         CREATE TABLE persona_preferences (id text PRIMARY KEY);",
    )
    .unwrap();
    converge_context_columns_sqlite(&conn).unwrap();
    assert!(columns(&conn, "beliefs").contains(&"context_as_of".to_owned()));
    converge_context_columns_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "beliefs")
            .iter()
            .filter(|c| *c == "context_as_of")
            .count(),
        1,
        "must not be added twice"
    );
}

#[test]
fn adds_only_the_missing_context_column() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE beliefs (id text PRIMARY KEY, context_cipher text);
         CREATE TABLE persona_stable_entries (id text PRIMARY KEY);",
    )
    .unwrap();
    converge_context_columns_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "beliefs")
            .iter()
            .filter(|c| *c == "context_cipher")
            .count(),
        1,
        "pre-existing column must not be added twice"
    );
    assert!(columns(&conn, "persona_stable_entries").contains(&"context_cipher".to_owned()));
}

#[test]
fn absent_context_tables_are_a_noop() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    converge_context_columns_sqlite(&conn).unwrap();
    let preference_cols = columns(&conn, "persona_preferences");
    assert!(preference_cols.is_empty(), "{preference_cols:?}");
    let stable_cols = columns(&conn, "persona_stable_entries");
    assert!(stable_cols.is_empty(), "{stable_cols:?}");
    let belief_cols = columns(&conn, "beliefs");
    assert!(belief_cols.is_empty(), "{belief_cols:?}");
}

/// A `distilled_units` table written before the reconciliation and evidence
/// columns gains all of them, and a second pass adds nothing. The convergence
/// has to work on a populated table, which is why every added column carries a
/// constant default: SQLite's ADD COLUMN refuses a non-constant one.
#[test]
fn adds_the_distilled_state_and_evidence_columns_and_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE distilled_units (
            bank_id text NOT NULL,
            item_id text NOT NULL,
            hub_id text NOT NULL,
            source_ids_json text NOT NULL DEFAULT '[]',
            marker text NOT NULL,
            record_version integer NOT NULL DEFAULT 1,
            distilled_at text NOT NULL,
            PRIMARY KEY (bank_id, item_id)
         );
         INSERT INTO distilled_units VALUES ('b', 'u', 'h', '[]', 'hub_distilled', 1, 't0');",
    )
    .unwrap();
    for column in [
        "revision",
        "stale_reason",
        "stale_detail",
        "stale_at",
        "evidence_json",
    ] {
        assert!(
            !columns(&conn, "distilled_units").contains(&column.to_owned()),
            "{column} must start absent"
        );
    }
    converge_distilled_state_sqlite(&conn).unwrap();
    for column in [
        "revision",
        "stale_reason",
        "stale_detail",
        "stale_at",
        "evidence_json",
    ] {
        assert!(
            columns(&conn, "distilled_units").contains(&column.to_owned()),
            "{column} must be added to an existing table"
        );
    }
    // The pre-existing row kept the column defaults: revision 1, no stale state,
    // and an empty evidence trail.
    let row: (i64, Option<String>, String) = conn
        .query_row(
            "SELECT revision, stale_reason, evidence_json FROM distilled_units
             WHERE bank_id = 'b' AND item_id = 'u'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("the pre-existing row");
    assert_eq!(row.0, 1, "a pre-v2 row reads revision 1");
    assert_eq!(row.1, None, "and no stale state");
    assert_eq!(row.2, "[]", "and an empty evidence trail");

    // Second pass must not error or duplicate a column.
    converge_distilled_state_sqlite(&conn).unwrap();
    assert_eq!(
        columns(&conn, "distilled_units")
            .iter()
            .filter(|c| *c == "evidence_json")
            .count(),
        1,
        "must not be added twice"
    );
}

#[test]
fn an_absent_distilled_units_table_is_a_noop() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    converge_distilled_state_sqlite(&conn).unwrap();
    let distilled_cols = columns(&conn, "distilled_units");
    assert!(distilled_cols.is_empty(), "{distilled_cols:?}");
}
