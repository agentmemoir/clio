// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres target measurement on schemas that are not
//! clio stores: tables without an `items` table measure fine and report no
//! banks.
//!
//! ## Owns
//! - The no-items measurement assertions
//!
//! ## Does not own
//! - The restore transaction's behavior (see `postgres_restore_tests`)
//! - The discovery classification (see `postgres_discovery_tests`)
//! - The restore report's row total (see `postgres_restore_rows_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The restore-report coverage lives in two siblings: the transaction's
//! behavior in `postgres_restore_tests`, the meaning of the report's
//! `rows_loaded` in `postgres_restore_rows_tests`.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres_restore_state::postgres_target_state;

/// Measuring a schema that holds tables but no store: the banks list is empty
/// because there is no `items` table, and the measurement still succeeds.
#[test]
fn measuring_a_schema_without_an_items_table_reports_no_banks() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&postgres_url_from_env());
    {
        let mut client =
            crate::pg_tls::connect(scoped.url()).expect("connect to the scoped schema");
        client
            .batch_execute("CREATE TABLE probe (id text primary key, note text)")
            .expect("create a plain table");
    }

    let state = postgres_target_state(scoped.url()).expect("measure a plain schema");

    assert_eq!(state.tables.len(), 1);
    assert_eq!(state.tables[0].name, "probe");
    assert!(state.banks.is_empty(), "{:?}", state.banks);
    assert!(
        state.is_empty(),
        "a schema without items or triples is empty"
    );
}
