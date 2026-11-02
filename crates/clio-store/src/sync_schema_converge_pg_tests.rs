// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres `sync_device_high_water` convergence: a
//! database whose bootstrap stamp is already current gains the table on the next
//! open, the convergence is idempotent, and a fresh database gets it from the
//! migrate batch as well.
//!
//! ## Owns
//! - Test assertions for `converge_pg`, including the already-stamped path
//!
//! ## Does not own
//! - Production convergence DDL (`sync_schema_converge`)
//! - The SQLite convergence assertions (`sync_schema_converge_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every case runs inside a
//! private schema, so it neither reads nor writes `public`.

use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::sync_schema_converge::converge_pg;
use crate::sync_store::SyncStore;

/// The shared Postgres URL, or `None` after the fail-closed skip line.
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres sync high water convergence tests", &err);
        return None;
    }
    Some(url)
}

/// Whether the table exists in the connection's own schema.
fn table_exists(client: &mut postgres::Client) -> bool {
    client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_name = 'sync_device_high_water')",
            &[],
        )
        .expect("table probe")
        .get(0)
}

/// The column names of the table, sorted in Rust so no catalog order is assumed.
fn columns(client: &mut postgres::Client) -> Vec<String> {
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'sync_device_high_water'",
            &[],
        )
        .expect("column read");
    let mut names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    names.sort();
    names
}

/// The columns a fresh database gets from the migrate batch, sorted.
///
/// The batch embeds the portable schema text, so this reads the declaration the
/// convergence statement restates rather than a list copied into this file. A
/// column added to one and not the other fails the comparison below instead of
/// drifting unnoticed.
fn fresh_columns(schema: &ScopedSchema) -> Vec<String> {
    drop(schema.open());
    let mut client = crate::pg_tls::connect(schema.url()).expect("connect");
    columns(&mut client)
}

/// A schema with no table gains it, and a second pass adds nothing.
#[test]
fn postgres_creates_the_table_and_is_idempotent() {
    let _shared = lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let fresh_schema = ScopedSchema::new(&url);
    let fresh = fresh_columns(&fresh_schema);
    let schema = ScopedSchema::new(&url);
    let mut client = crate::pg_tls::connect(schema.url()).expect("connect");
    assert!(!table_exists(&mut client), "the table must start absent");

    converge_pg(&mut client).expect("converge");
    assert!(table_exists(&mut client), "the step must create the table");
    assert_eq!(
        columns(&mut client),
        fresh,
        "the converged shape must match the fresh declaration"
    );

    // A second pass must not error or change the shape.
    converge_pg(&mut client).expect("converge again");
    assert_eq!(columns(&mut client).len(), fresh.len());
}

/// The reported failure: a database whose bootstrap stamp is already current
/// skips the compile-time batch, so the convergence step is the only path that
/// creates the table. A store opened on such a database serves the new methods.
#[test]
fn postgres_an_already_stamped_database_gains_the_table() {
    let _shared = lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let schema = ScopedSchema::new(&url);
    // First open: the batch stamps both versions and creates the table.
    drop(schema.open());
    // Remove only the table, leaving both stamps current: this is exactly the
    // state an already-bootstrapped database is in before the table existed.
    schema
        .execute("DROP TABLE sync_device_high_water")
        .expect("drop the table under a current stamp");
    assert_eq!(
        schema
            .query_value(
                "SELECT value FROM schema_settings WHERE key = 'postgres_bootstrap_version'"
            )
            .expect("bootstrap stamp"),
        Some("1".to_owned()),
        "the bootstrap stamp must stay current, or this test proves nothing"
    );
    assert_eq!(
        schema
            .query_value(
                "SELECT (to_regclass(current_schema() || '.sync_device_high_water') IS NULL)::text"
            )
            .expect("table presence"),
        Some("true".to_owned()),
        "the table must be absent before the second open, or this test proves nothing"
    );

    // Second open: the batch is skipped, so only the convergence step can create
    // the table.
    let store = schema.open();
    assert_eq!(
        schema
            .query_value(
                "SELECT (to_regclass(current_schema() || '.sync_device_high_water') IS NOT NULL)::text"
            )
            .expect("table presence"),
        Some("true".to_owned()),
        "the table must exist after the second open"
    );
    store
        .device_high_water_set("b-converge", "dev-a", "item", "2026-01-01T00:00:00Z")
        .expect("the converged table serves the write");
    assert_eq!(
        store
            .device_high_water_get("b-converge", "dev-a", "item")
            .expect("the converged table serves the read"),
        Some("2026-01-01T00:00:00Z".to_owned())
    );
}

/// A fresh database gets the table from the migrate batch, and the convergence
/// step that follows it is a no-op rather than an error.
#[test]
fn postgres_a_fresh_database_gets_the_table_from_both_paths() {
    let _shared = lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let fresh_schema = ScopedSchema::new(&url);
    let fresh = fresh_columns(&fresh_schema);
    let schema = ScopedSchema::new(&url);
    drop(schema.open());
    let mut client = crate::pg_tls::connect(schema.url()).expect("connect");
    assert!(
        table_exists(&mut client),
        "the migrate batch must create the table on a fresh database"
    );
    converge_pg(&mut client).expect("the step after the batch must be a no-op");
    assert_eq!(
        columns(&mut client),
        fresh,
        "the batch and the convergence step must declare the same shape"
    );
}
