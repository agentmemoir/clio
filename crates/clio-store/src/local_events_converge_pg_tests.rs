// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres local-event convergence: a scratch database
//! whose `local_events` table still has the old placeholder shape is brought
//! onto the event columns when the store opens, so the real writer lands a
//! record and reads it back.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - The converged column, nullability, and index assertions
//!
//! ## Does not own
//! - The convergence implementation (`local_events_converge`)
//! - The event write and read contract (`local_events`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`local_events_converge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres repair of a placeholder `local_events`
//! table changes. A fresh database's table shape is pinned by the
//! `migrate_compliance_slots_tests` shape test.
//!
//! Test-only. Never compiled into production builds.

use crate::db_url::{postgres_url_from_env, with_database};
use crate::dek::{DekProvider, LocalDevKms};
use crate::local_events::{LocalEventIntent, LocalEventState, LocalEventStore};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::AmError;
use std::sync::Arc;

/// The old placeholder shape: one key and no event column.
const PLACEHOLDER_DDL: &str = "CREATE TABLE local_events (id text PRIMARY KEY);";

/// The shared Postgres URL, or `None` after the fail-closed skip line: an
/// unreachable Postgres fails the suite unless `CLIO_SKIP_PG_TESTS` opts out.
fn shared_url_or_skip() -> Option<String> {
    let url = postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres local event convergence tests", &err);
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic.
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("clio_events_{label}_{}_{nanos}", std::process::id());
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
        if let Ok(mut admin) = crate::pg_tls::connect(&postgres_url_from_env()) {
            let _ = admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name));
        }
    }
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    with_database(&postgres_url_from_env(), name)
}

/// Open the store on the scratch database.
fn open_pg(url: &str) -> PostgresStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(url, kms).expect("open a scratch postgres store")
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

/// The `local_events` shape the assertions read: the column names in ordinal
/// order, whether the legacy `id` column is nullable, and the index names.
fn shape(store: &PostgresStore) -> (Vec<String>, bool, Vec<String>) {
    store
        .with_client(|client| {
            let columns = client
                .query(
                    "SELECT column_name FROM information_schema.columns
                     WHERE table_schema = current_schema()
                       AND table_name = 'local_events'
                     ORDER BY ordinal_position",
                    &[],
                )
                .am("local events columns")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            let legacy_nullable = client
                .query_one(
                    "SELECT is_nullable = 'YES' FROM information_schema.columns
                     WHERE table_schema = current_schema()
                       AND table_name = 'local_events'
                       AND column_name = 'id'",
                    &[],
                )
                .am("local events legacy key nullability")?
                .get::<_, bool>(0);
            let indexes = client
                .query(
                    "SELECT indexname FROM pg_indexes
                     WHERE schemaname = current_schema()
                       AND tablename = 'local_events'",
                    &[],
                )
                .am("local events indexes")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            Ok((columns, legacy_nullable, indexes))
        })
        .expect("read the local_events shape")
}

/// A database created under the old placeholder shape converges when the store
/// opens: the writer's own insert and update name the event columns, and the
/// legacy key keeps its column but loses the NOT NULL that refused every row.
#[test]
fn a_pre_existing_placeholder_table_converges_on_open() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "placeholder");
    drop(admin);
    let db_url = url_for_database(&db.name);
    let mut legacy = crate::pg_tls::connect(&db_url).expect("legacy connect");
    legacy
        .batch_execute(PLACEHOLDER_DDL)
        .expect("the old placeholder shape");
    drop(legacy);

    let store = open_pg(&db_url);
    want(
        store.record_intent(&intent("act-1", "2026-03-01T00:00:00Z")),
        "intent on a converged table",
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

    let expected: Vec<String> = std::iter::once("id".to_owned())
        .chain(
            super::EVENT_COLUMNS
                .iter()
                .map(|(column, _)| (*column).to_owned()),
        )
        .collect();
    let (columns, legacy_nullable, indexes) = shape(&store);
    assert_eq!(
        columns, expected,
        "the placeholder table converges onto the event columns"
    );
    assert!(
        legacy_nullable,
        "the legacy key keeps its column and drops NOT NULL"
    );
    let wanted = ["local_events_action_id_uidx", "local_events_seq_uidx"];
    for index in wanted {
        assert!(
            indexes.iter().any(|name| name == index),
            "the converged table backs {index}: {indexes:?}"
        );
    }

    let duplicate = store.with_client(|client| {
        client
            .batch_execute(
                "INSERT INTO local_events
                    (action_id, seq, actor, scope, kind, event_time, outcome_state)
                 VALUES ('act-dup', 1, 'tester', 'scope-opaque', 'erase_request',
                         '2026-03-01T00:00:01Z', 'allowed')",
            )
            .am("duplicate order position insert")
    });
    let refused = duplicate.expect_err("the repaired table refuses a duplicate order position");
    assert!(
        refused.to_string().contains("local_events_seq_uidx"),
        "the seq index refuses the order position: {refused}"
    );

    drop(store);
    let reopened = open_pg(&db_url);
    let (columns, legacy_nullable, indexes) = shape(&reopened);
    assert_eq!(
        columns, expected,
        "a second open leaves the converged columns in place and in order"
    );
    assert!(
        legacy_nullable,
        "a second open leaves the legacy key nullable"
    );
    for index in wanted {
        assert!(
            indexes.iter().any(|name| name == index),
            "a second open keeps {index}: {indexes:?}"
        );
    }
}
