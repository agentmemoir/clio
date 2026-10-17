// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite local-event convergence: a file-backed
//! database whose `local_events` table still has the old placeholder shape is
//! rebuilt onto the eight event columns when the store opens, so the real
//! writer lands a record and the table matches a fresh one.
//!
//! ## Owns
//! - The scratch-directory fixture and the placeholder-shape setup
//! - The converged column, index, and row assertions
//!
//! ## Does not own
//! - The convergence implementation (`local_events_converge`)
//! - The event write and read contract (`local_events`)
//! - The Postgres backend assertions (`local_events_converge_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the SQLite repair of a placeholder `local_events`
//! table changes. A fresh database's table shape is pinned by the
//! `migrate_compliance_slots_tests` shape test.
//!
//! Test-only. Never compiled into production builds.

use crate::erase_test_support::local_kms;
use crate::local_events::{LocalEventIntent, LocalEventState, LocalEventStore};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::AmError;

/// The old placeholder shape: one key and no event column.
const PLACEHOLDER_DDL: &str = "CREATE TABLE local_events (id text PRIMARY KEY);";

/// A scratch directory removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir(
            "clio-store-local-events-converge",
        ))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write the placeholder table, plus whatever the case adds to it.
fn placeholder(path: &std::path::Path, extra: &str) {
    let legacy = rusqlite::Connection::open(path).expect("open the placeholder database");
    legacy
        .execute_batch(PLACEHOLDER_DDL)
        .expect("the old placeholder shape");
    if !extra.is_empty() {
        legacy.execute_batch(extra).expect("the placeholder rows");
    }
}

/// An intent with fixed field values.
fn intent<'a>(action_id: &'a str, event_time: &'a str) -> LocalEventIntent<'a> {
    LocalEventIntent {
        action_id,
        actor: "tester",
        scope: "scope-opaque",
        kind: "erase_request",
        event_time,
    }
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// The `local_events` columns, in the table's own order.
fn columns(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(local_events)")
                .am("local events table_info")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("local events table_info rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("local events table_info rows")?;
            Ok(names)
        })
        .expect("read the local_events columns")
}

/// The `local_events` index names.
fn index_names(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA index_list(local_events)")
                .am("local events index list")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("local events index list rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("local events index list rows")?;
            Ok(names)
        })
        .expect("read the local_events indexes")
}

/// The eight event column names, in the order the DDL declares them.
fn expected_columns() -> Vec<String> {
    super::EVENT_COLUMNS
        .iter()
        .map(|(column, _)| (*column).to_owned())
        .collect()
}

/// Both unique index names a converged table carries.
const INDEXES: [&str; 2] = ["local_events_action_id_uidx", "local_events_seq_uidx"];

/// The row count of `local_events`, read without the event shape.
fn row_count(store: &SqliteStore) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM local_events", [], |row| row.get(0))
                .am("count the local event rows")
        })
        .expect("count the local event rows")
}

/// A database created under the old placeholder shape converges when the store
/// opens: the table ends as exactly the fresh DDL's eight columns, both unique
/// indexes, and the writer's own insert and reads work.
#[test]
fn a_pre_existing_placeholder_table_converges_on_open() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    placeholder(&path, "");
    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    want(
        store.record_intent(&intent("act-1", "2026-03-01T00:00:00Z")),
        "intent on a converged table",
    );
    assert_eq!(
        store.pending_actions().expect("pending actions"),
        vec!["act-1".to_owned()],
        "the intent is pending"
    );
    want(
        store.record_outcome("act-1", false, "erased"),
        "outcome on a converged table",
    );
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "one converged row: {events:?}");
    assert_eq!(events[0].action_id, "act-1", "the action id round-trips");
    assert_eq!(
        events[0].state,
        LocalEventState::Allowed,
        "the outcome lands"
    );
    assert_eq!(
        events[0].result.as_deref(),
        Some("erased"),
        "the outcome text round-trips"
    );
    assert!(events[0].seq >= 1, "the store assigns an order position");

    let repaired = columns(&store);
    assert_eq!(
        repaired,
        expected_columns(),
        "the rebuilt table holds exactly the event columns, in DDL order"
    );
    assert!(
        !repaired.iter().any(|name| name == "id"),
        "the legacy key is gone: {repaired:?}"
    );
    let indexes = index_names(&store);
    for index in INDEXES {
        assert!(
            indexes.iter().any(|name| name == index),
            "the converged table backs {index}: {indexes:?}"
        );
    }

    let duplicate = store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO local_events
                (action_id, seq, actor, scope, kind, event_time, outcome_state)
             VALUES ('act-dup', ?1, 'tester', 'scope-opaque', 'erase_request',
                     '2026-03-01T00:00:01Z', 'allowed')",
            rusqlite::params![events[0].seq],
        )
        .am("duplicate order position insert")
    });
    let refused = duplicate.expect_err("the repaired table refuses a duplicate order position");
    assert!(
        refused.to_string().contains("UNIQUE constraint failed"),
        "the seq index refuses the order position: {refused}"
    );

    // A fresh database's table is the shape the repair has to reach.
    let fresh = SqliteStore::open(&scratch.0.join("fresh.db"), local_kms())
        .expect("open a fresh sqlite database");
    assert_eq!(
        columns(&fresh),
        repaired,
        "the repaired columns equal the columns a fresh database declares"
    );
    drop(fresh);
    drop(store);

    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen the converged database");
    assert_eq!(
        columns(&reopened),
        repaired,
        "a second open leaves the converged columns in place and in order"
    );
    let indexes = index_names(&reopened);
    for index in INDEXES {
        assert!(
            indexes.iter().any(|name| name == index),
            "a second open keeps {index}: {indexes:?}"
        );
    }
}

/// A placeholder row holds no event, so the rebuild does not copy it: the
/// ordered read works afterwards, and the writer lands its own record.
#[test]
fn a_placeholder_row_with_no_event_values_is_removed_on_converge() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    placeholder(&path, "INSERT INTO local_events VALUES ('legacy');");
    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    let events = store
        .list_events()
        .expect("list events on the converged table");
    assert!(
        events.is_empty(),
        "the row that carries no event is gone: {events:?}"
    );
    want(
        store.record_intent(&intent("act-2", "2026-03-02T00:00:00Z")),
        "intent after the placeholder row is removed",
    );
    let events = store.list_events().expect("list events after the write");
    assert_eq!(events.len(), 1, "the writer lands one row: {events:?}");
    assert_eq!(
        events[0].action_id, "act-2",
        "the new action id round-trips"
    );
    assert_eq!(
        columns(&store),
        expected_columns(),
        "the rebuilt table holds exactly the event columns, in DDL order"
    );
}

/// Only a row whose eight event columns are all NULL is removed. A row that
/// already holds an event value is copied, so the repair never deletes one.
#[test]
fn a_placeholder_row_with_an_event_value_is_kept() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    placeholder(
        &path,
        "ALTER TABLE local_events ADD COLUMN actor text;
         INSERT INTO local_events (id, actor) VALUES ('legacy', 'someone');",
    );
    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    assert_eq!(row_count(&store), 1, "the row with an event value is kept");
    let actor: Option<String> = store
        .with_conn(|conn| {
            conn.query_row("SELECT actor FROM local_events", [], |row| row.get(0))
                .am("read the kept row")
        })
        .expect("read the kept row");
    assert_eq!(
        actor.as_deref(),
        Some("someone"),
        "the copied event value round-trips"
    );
}

/// A table that already lost the legacy key is repaired in place: it gains the
/// unique indexes it lacks and keeps its declared key and the rows it holds.
#[test]
fn a_table_without_the_legacy_key_keeps_its_primary_key() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let legacy = rusqlite::Connection::open(&path).expect("open the placeholder database");
    legacy
        .execute_batch(
            "CREATE TABLE local_events
                 (action_id text PRIMARY KEY, seq bigint, actor text, scope text,
                  kind text, event_time text, outcome_state text, result text);
             INSERT INTO local_events
                 (action_id, seq, actor, scope, kind, event_time, outcome_state)
             VALUES ('act-old', 7, 'tester', 'scope-opaque', 'erase_request',
                     '2026-03-03T00:00:00Z', 'pending');",
        )
        .expect("the table that lost the legacy key");
    drop(legacy);

    let store = SqliteStore::open(&path, local_kms()).expect("open a table without the legacy key");
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "the existing row is kept: {events:?}");
    assert_eq!(events[0].seq, 7, "its order position is untouched");
    assert_eq!(
        columns(&store),
        expected_columns(),
        "the column list stays the event columns, in DDL order"
    );
    let indexes = index_names(&store);
    for index in INDEXES {
        assert!(
            indexes.iter().any(|name| name == index),
            "the repair adds {index}: {indexes:?}"
        );
    }
    let key: i64 = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT pk FROM pragma_table_info('local_events') WHERE name = 'action_id'",
                [],
                |row| row.get(0),
            )
            .am("read the action_id key")
        })
        .expect("read the action_id key");
    assert_eq!(key, 1, "the primary key the table declared stays");
}
