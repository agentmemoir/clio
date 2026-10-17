// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres local-event convergence: a private schema
//! whose `local_events` table still has the old placeholder shape is brought
//! onto the event columns when the store opens, so the real writer lands a
//! record, reads it back, and the table matches a fresh one.
//!
//! ## Owns
//! - The private-schema fixture and its placeholder-shape setup
//! - The converged column, index, and row assertions
//! - The dump-and-restore round trip of a repaired store
//!
//! ## Does not own
//! - The convergence implementation (`local_events_converge`)
//! - The event write and read contract (`local_events`)
//! - Schema creation and drop (`pg_scoped_schema`)
//! - The SQLite backend assertions (`local_events_converge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres repair of a placeholder `local_events`
//! table changes. A fresh database's table shape is pinned by the
//! `migrate_compliance_slots_tests` shape test.
//!
//! Test-only. Never compiled into production builds.

use crate::db_url::postgres_url_from_env;
use crate::local_events::{LocalEventIntent, LocalEventState, LocalEventStore};
use crate::migrate::{IntoAmError, map_db_err};
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use crate::postgres_backup_model::PgTable;
use crate::postgres_dump::{PostgresDumpFacts, snapshot_postgres_into};
use crate::postgres_restore::{PostgresRestoreRequest, RestoreTable, restore_postgres};
use clio_types::AmError;
use std::path::PathBuf;

/// The old placeholder shape: one key and no event column.
const PLACEHOLDER_DDL: &str = "CREATE TABLE local_events (id text PRIMARY KEY);";

/// Both unique index names a converged table carries.
const INDEXES: [&str; 2] = ["local_events_action_id_uidx", "local_events_seq_uidx"];

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

/// Create the placeholder table in the schema, plus whatever the case adds.
fn placeholder(schema: &ScopedSchema, extra: &str) {
    schema
        .execute(PLACEHOLDER_DDL)
        .expect("the old placeholder shape");
    if !extra.is_empty() {
        schema.execute(extra).expect("the placeholder rows");
    }
}

/// The schema the store's own connection resolves to.
fn schema_name(schema: &ScopedSchema) -> String {
    schema
        .query_value("SELECT current_schema()::text")
        .expect("read the scoped schema")
        .expect("the scoped search_path resolves to a schema")
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

/// The eight event column names, in the order the DDL declares them.
fn expected_columns() -> Vec<String> {
    super::EVENT_COLUMNS
        .iter()
        .map(|(column, _)| (*column).to_owned())
        .collect()
}

/// The `local_events` shape the assertions read: the column names in ordinal
/// order and the index names.
fn shape(store: &PostgresStore) -> (Vec<String>, Vec<String>) {
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
            Ok((columns, indexes))
        })
        .expect("read the local_events shape")
}

/// Assert the repaired column list and both unique indexes.
fn assert_converged(store: &PostgresStore) {
    let (columns, indexes) = shape(store);
    assert_eq!(
        columns,
        expected_columns(),
        "the table holds exactly the event columns, in DDL order"
    );
    assert!(
        !columns.iter().any(|name| name == "id"),
        "the legacy key is gone: {columns:?}"
    );
    for index in INDEXES {
        assert!(
            indexes.iter().any(|name| name == index),
            "the table backs {index}: {indexes:?}"
        );
    }
}

/// A scratch directory for staged dump members, removed when the guard drops.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir("clio-local-events-restore"))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Dump `url`'s store into one staged file per table, in member order.
fn dump_into_scratch(url: &str) -> (PostgresDumpFacts, Vec<PathBuf>, ScratchDir) {
    let dir = ScratchDir::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let facts = snapshot_postgres_into(url, &mut |table: &PgTable, ordinal: usize| {
        let path = dir.0.join(format!("{ordinal:03}_{}.txt", table.name));
        let file = std::fs::File::create(&path)
            .map_err(|error| map_db_err("stage a dump member", error))?;
        paths.push(path.clone());
        Ok((path, file))
    })
    .expect("dump the repaired store");
    (facts, paths, dir)
}

/// The restore input the archive's facts describe.
fn restore_tables(facts: &PostgresDumpFacts, paths: &[PathBuf]) -> Vec<RestoreTable> {
    facts
        .tables
        .iter()
        .zip(paths)
        .map(|(table, path)| RestoreTable {
            table: table.table.clone(),
            columns: table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            member_path: path.clone(),
            rows: table.rows,
        })
        .collect()
}

/// A schema created under the old placeholder shape converges when the store
/// opens: the table ends as exactly the fresh DDL's eight columns with both
/// unique indexes, and the writer's own insert and reads work.
#[test]
fn a_pre_existing_placeholder_table_converges_on_open() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    placeholder(&source, "");

    let store = source.open();
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
    assert_converged(&store);

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

    // A fresh schema's table is the shape the repair has to reach.
    let fresh = ScopedSchema::new(&url);
    let fresh_store = fresh.open();
    assert_eq!(
        shape(&fresh_store).0,
        shape(&store).0,
        "the repaired columns equal the columns a fresh database declares"
    );
    drop(fresh_store);

    drop(store);
    let reopened = source.open();
    assert_converged(&reopened);
}

/// A placeholder row holds no event, so the repair deletes it: the ordered read
/// works afterwards, and the writer lands its own record.
#[test]
fn a_placeholder_row_with_no_event_values_is_removed_on_converge() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    placeholder(&source, "INSERT INTO local_events VALUES ('legacy');");

    let store = source.open();
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
    assert_converged(&store);
}

/// A table that already lost the legacy key is repaired in place: it gains both
/// unique indexes and keeps the primary key it declared and the rows it holds.
#[test]
fn a_table_without_the_legacy_key_keeps_its_primary_key() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    source
        .execute(
            "CREATE TABLE local_events
                 (action_id text PRIMARY KEY, seq bigint, actor text, scope text,
                  kind text, event_time text, outcome_state text, result text);",
        )
        .expect("the table that lost the legacy key");

    let store = source.open();
    want(
        store.record_intent(&intent("act-3", "2026-03-03T00:00:00Z")),
        "intent on a table without the legacy key",
    );
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "the writer lands one row: {events:?}");
    assert_converged(&store);
    let keys: i64 = store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT count(*) FROM pg_constraint
                     WHERE conrelid = to_regclass('local_events') AND contype = 'p'",
                    &[],
                )
                .am("read the local events primary key")
                .map(|row| row.get(0))
        })
        .expect("read the local events primary key");
    assert_eq!(keys, 1, "the primary key the table declared stays");
}

/// A repaired store dumps and restores into a fresh schema: the archive's
/// column list now matches the target's, so the restore loads the event row
/// instead of refusing the archive for the missing legacy key.
#[test]
fn a_repaired_store_restores_into_a_fresh_database() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    placeholder(&source, "INSERT INTO local_events VALUES ('legacy');");
    let store = source.open();
    want(
        store.record_intent(&intent("act-restored", "2026-03-04T00:00:00Z")),
        "intent before the dump",
    );
    want(
        store.record_outcome("act-restored", true, "refused"),
        "outcome before the dump",
    );
    drop(store);

    // The target schema holds a fresh store: the DDL declares its own table.
    let target = ScopedSchema::new(&url);
    drop(target.open());
    let (facts, paths, _staged) = dump_into_scratch(source.url());
    let report = restore_postgres(&PostgresRestoreRequest {
        url: target.url().to_owned(),
        schema: schema_name(&target),
        tables: restore_tables(&facts, &paths),
    })
    .expect("restore a repaired store into a fresh schema");
    assert!(
        report.counts_match,
        "the restored counts match the archive: {report:?}"
    );

    let mut client = crate::pg_tls::connect(target.url()).expect("target read connect");
    let rows = client
        .query(
            "SELECT action_id, outcome_state, result FROM local_events",
            &[],
        )
        .expect("read the restored local events");
    assert_eq!(rows.len(), 1, "the restored table holds the event row");
    assert_eq!(
        rows[0].get::<_, String>(0),
        "act-restored",
        "the action id round-trips"
    );
    assert_eq!(
        rows[0].get::<_, String>(1),
        "denied",
        "the outcome state round-trips"
    );
    assert_eq!(
        rows[0].get::<_, Option<String>>(2).as_deref(),
        Some("refused"),
        "the result text round-trips"
    );
}
