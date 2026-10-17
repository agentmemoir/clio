// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres copy-inventory convergence: a private schema
//! whose `copy_inventory` table still has the old placeholder shape is brought
//! onto the metadata columns on first use, so the real save lands a row and the
//! table matches a fresh one.
//!
//! ## Owns
//! - The private-schema fixture and its placeholder-shape setup
//! - The converged column, index, and row assertions
//!
//! ## Does not own
//! - The convergence implementation (`copy_inventory_converge`)
//! - The save/load contract (`copy_inventory`)
//! - The SQLite backend assertions (`copy_inventory_converge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres repair of a placeholder `copy_inventory`
//! table changes. A fresh database's table shape is pinned by the
//! `migrate_compliance_slots_tests` shape test.
//!
//! Test-only. Never compiled into production builds.

use crate::copy_inventory::{CopyInventoryRow, CopyInventoryStore};
use crate::migrate::IntoAmError;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use clio_types::AmError;

/// The old placeholder shape: one key and no metadata column.
const PLACEHOLDER_DDL: &str = "CREATE TABLE copy_inventory (id text PRIMARY KEY);";

/// The unique index name a converged table carries.
const INDEX: &str = "copy_inventory_copy_id_uidx";

/// The shared Postgres URL, or `None` after the fail-closed skip line: an
/// unreachable Postgres fails the suite unless `CLIO_SKIP_PG_TESTS` opts out.
fn shared_url_or_skip() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres copy inventory convergence tests", &err);
        return None;
    }
    Some(url)
}

/// A copy row with fixed field values.
fn row(copy_id: &str) -> CopyInventoryRow {
    CopyInventoryRow {
        copy_id: copy_id.to_owned(),
        request_id: "req-1".to_owned(),
        custody: "staged".to_owned(),
        owner: "clio".to_owned(),
        required_action: "seal".to_owned(),
        state: "inventoried".to_owned(),
        exception_reason: None,
        updated_at: "2026-03-01T00:00:00Z".to_owned(),
    }
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// The eight metadata column names, in the order the DDL declares them.
fn expected_columns() -> Vec<String> {
    super::COPY_COLUMNS
        .iter()
        .map(|(column, _)| (*column).to_owned())
        .collect()
}

/// The `copy_inventory` columns in ordinal order and its index names.
fn shape(store: &PostgresStore) -> (Vec<String>, Vec<String>) {
    store
        .with_client(|client| {
            let columns = client
                .query(
                    "SELECT column_name FROM information_schema.columns
                     WHERE table_schema = current_schema()
                       AND table_name = 'copy_inventory'
                     ORDER BY ordinal_position",
                    &[],
                )
                .am("copy inventory columns")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            let indexes = client
                .query(
                    "SELECT indexname FROM pg_indexes
                     WHERE schemaname = current_schema() AND tablename = 'copy_inventory'",
                    &[],
                )
                .am("copy inventory indexes")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            Ok((columns, indexes))
        })
        .expect("read the copy_inventory shape")
}

/// The row count of `copy_inventory`.
fn row_count(store: &PostgresStore) -> i64 {
    store
        .with_client(|client| {
            client
                .query_one("SELECT count(*) FROM copy_inventory", &[])
                .am("copy inventory row count")
                .map(|row| row.get(0))
        })
        .expect("count the copy inventory rows")
}

/// A schema created under the old placeholder shape converges on first use: the
/// table ends as exactly the fresh DDL's eight columns with the unique index, the
/// placeholder row that carries no metadata is gone, and the save whose insert
/// the legacy key would have refused lands.
#[test]
fn a_pre_existing_placeholder_table_converges_on_first_use() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url_or_skip() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    source
        .execute(PLACEHOLDER_DDL)
        .expect("the old placeholder shape");
    source
        .execute("INSERT INTO copy_inventory VALUES ('legacy');")
        .expect("the placeholder row");

    let store = source.open();
    want(store.save(&row("copy-1")), "save on a converged table");
    want(store.save(&row("copy-1")), "retry on a converged table");
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(row("copy-1")),
        "the converged table reads back"
    );
    let (columns, indexes) = shape(&store);
    assert_eq!(
        columns,
        expected_columns(),
        "the table holds exactly the metadata columns, in DDL order"
    );
    assert!(
        !columns.iter().any(|name| name == "id"),
        "the legacy key is gone: {columns:?}"
    );
    assert!(
        indexes.iter().any(|name| name == INDEX),
        "the table backs {INDEX}: {indexes:?}"
    );
    assert_eq!(
        row_count(&store),
        1,
        "the row that carries no metadata is gone and only the saved row is left"
    );

    // A fresh schema's table is the shape the repair has to reach.
    let fresh = ScopedSchema::new(&url);
    let fresh_store = fresh.open();
    assert_eq!(
        shape(&fresh_store).0,
        columns,
        "the repaired columns equal the columns a fresh database declares"
    );
}
