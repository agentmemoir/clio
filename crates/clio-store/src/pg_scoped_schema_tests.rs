// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the scoped-schema harness's raw-SQL helpers: the execute
//! and query-value calls other crates' tests run through.
//!
//! ## Owns
//! - The assertions that a raw statement runs and that a query helper reads one
//!   value back, including the environment-URL fallback
//!
//! ## Does not own
//! - The harness itself (see `pg_scoped_schema`)
//! - The store and backup halves the harness is used to test
//!
//! ## Boundary
//!
//! Test-only, and every test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::*;

#[test]
fn execute_runs_sql_and_query_value_reads_one_value_back() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&postgres_url_from_env());
    scoped
        .execute("CREATE TABLE probe (id text primary key)")
        .expect("create a probe table");
    scoped
        .execute("INSERT INTO probe (id) VALUES ('one')")
        .expect("insert one row");
    let value = scoped
        .query_value("SELECT id::text FROM probe LIMIT 1")
        .expect("read the value back");
    assert_eq!(value.as_deref(), Some("one"));
    let missing = scoped
        .query_value("SELECT id::text FROM probe WHERE id = 'absent'")
        .expect("an absent row reads as None");
    assert_eq!(missing, None);
    assert!(scoped.execute("DROP TABLE probe").is_ok());
    // A failing statement is an error, not a panic.
    assert!(scoped.execute("SELECT * FROM probe").is_err());
    assert!(scoped.query_value("SELECT * FROM probe").is_err());
}
