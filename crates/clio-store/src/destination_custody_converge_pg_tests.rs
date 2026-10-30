// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres export-destination custody column
//! convergence: a pre-existing table gains both columns, the convergence is
//! idempotent, and an absent table is a no-op.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - Test assertions for `converge_pg`
//!
//! ## Does not own
//! - Production convergence SQL (`destination_custody_converge`)
//! - The SQLite convergence assertions (`destination_custody_converge_tests`)
//! - Shared-Postgres test serialization (`pg_lock`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::destination_custody_converge::converge_pg;

/// The shared Postgres URL, or `None` after the fail-closed skip line.
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip(
            "postgres export destination convergence tests",
            &err,
        );
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic (the `destination_custody_pg_tests` fixture shape).
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("clio_destconv_{label}_{}_{nanos}", std::process::id());
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

/// The column names of the table, sorted in Rust so no catalog order is
/// assumed; `None` when the table is absent.
fn columns(admin: &mut postgres::Client, table: &str) -> Option<Vec<String>> {
    let exists: bool = admin
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_name = $1)",
            &[&table],
        )
        .expect("table probe")
        .get(0);
    if !exists {
        return None;
    }
    let rows = admin
        .query(
            "SELECT column_name FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = $1",
            &[&table],
        )
        .expect("column read");
    let mut names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    names.sort();
    Some(names)
}

/// A legacy table without the copy-custody columns gains both, and a second
/// pass adds nothing.
#[test]
fn postgres_adds_the_copy_custody_columns_and_is_idempotent() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "converge");
    drop(admin);
    let mut client = crate::pg_tls::connect(&url_for_database(&db.name)).expect("scratch connect");
    client
        .batch_execute(
            "CREATE TABLE export_destinations (
                action_id text PRIMARY KEY,
                destination_path text NOT NULL,
                custody_owner text NOT NULL,
                required_action text NOT NULL,
                state text NOT NULL,
                exception_reason text,
                updated_at text NOT NULL
             );",
        )
        .expect("create the legacy table");
    converge_pg(&mut client).expect("converge");
    let cols = columns(&mut client, "export_destinations").expect("the table");
    assert!(cols.contains(&"copy_digest".to_owned()), "{cols:?}");
    assert!(cols.contains(&"receiver".to_owned()), "{cols:?}");
    // Second pass must not error or duplicate a column.
    converge_pg(&mut client).expect("converge again");
    let cols = columns(&mut client, "export_destinations").expect("the table");
    assert_eq!(
        cols.iter()
            .filter(|c| *c == "copy_digest" || *c == "receiver")
            .count(),
        2,
        "{cols:?}"
    );
}

/// A database without the table is a no-op: the slot DDL declares the columns
/// itself on a fresh database.
#[test]
fn postgres_an_absent_export_destinations_table_is_a_noop() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "absent");
    drop(admin);
    let mut client = crate::pg_tls::connect(&url_for_database(&db.name)).expect("scratch connect");
    converge_pg(&mut client).expect("converge");
    assert!(
        columns(&mut client, "export_destinations").is_none(),
        "no table must be created"
    );
}
