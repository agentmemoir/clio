// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite local-event convergence: a file-backed
//! database whose `local_events` table still has the old placeholder shape is
//! brought onto the event columns when the store opens, so the real writer
//! lands a record.
//!
//! ## Owns
//! - The scratch-directory fixture and the placeholder-shape setup
//! - The converged column and index assertions
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

/// The `local_events` columns, in `PRAGMA table_info` order.
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

/// A database created under the old placeholder shape converges when the store
/// opens: the writer's own insert and queries name the event columns, and the
/// legacy key stays in place with its uniqueness restored on `action_id`.
#[test]
fn a_pre_existing_placeholder_table_converges_on_open() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let legacy = rusqlite::Connection::open(&path).expect("open the placeholder database");
    legacy
        .execute_batch(PLACEHOLDER_DDL)
        .expect("the old placeholder shape");
    drop(legacy);

    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    want(
        store.record_intent(&intent("act-1", "2026-03-01T00:00:00Z")),
        "intent on a converged table",
    );
    let pending = store.pending_actions().expect("pending actions");
    assert_eq!(pending, vec!["act-1".to_owned()], "the intent is pending");

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

    let mut expected = vec!["id".to_owned()];
    expected.extend(
        super::EVENT_COLUMNS
            .iter()
            .map(|(column, _)| (*column).to_owned()),
    );
    assert_eq!(
        columns(&store),
        expected,
        "the placeholder table converges onto the event columns"
    );
    assert!(
        index_names(&store)
            .iter()
            .any(|name| name == "local_events_action_id_uidx"),
        "the converged table backs action_id with a unique index"
    );
}
