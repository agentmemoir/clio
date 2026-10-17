// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite copy-inventory convergence: a file-backed
//! database whose `copy_inventory` table still has the old placeholder shape is
//! rebuilt onto the eight metadata columns on first use, so the real save lands
//! a row and the table matches a fresh one.
//!
//! ## Owns
//! - The scratch-directory fixture and the placeholder-shape setup
//! - The converged column, index, and row assertions
//!
//! ## Does not own
//! - The convergence implementation (`copy_inventory_converge`)
//! - The save/load contract (`copy_inventory`)
//! - The Postgres backend assertions (`copy_inventory_converge_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the SQLite repair of a placeholder `copy_inventory`
//! table changes. A fresh database's table shape is pinned by the
//! `migrate_compliance_slots_tests` shape test.
//!
//! Test-only. Never compiled into production builds.

use crate::copy_inventory::{CopyInventoryRow, CopyInventoryStore};
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::AmError;

/// The old placeholder shape: one key and no metadata column.
const PLACEHOLDER_DDL: &str = "CREATE TABLE copy_inventory (id text PRIMARY KEY);";

/// The unique index name a converged table carries.
const INDEX: &str = "copy_inventory_copy_id_uidx";

/// A scratch directory removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir(
            "clio-store-copy-inventory-converge",
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

/// The `copy_inventory` columns, in the table's own order.
fn columns(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(copy_inventory)")
                .am("copy inventory table_info")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("copy inventory table_info rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("copy inventory table_info rows")?;
            Ok(names)
        })
        .expect("read the copy_inventory columns")
}

/// The `copy_inventory` index names.
fn index_names(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA index_list(copy_inventory)")
                .am("copy inventory index list")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("copy inventory index list rows")?
                .collect::<Result<Vec<_>, _>>()
                .am("copy inventory index list rows")?;
            Ok(names)
        })
        .expect("read the copy_inventory indexes")
}

/// The row count of `copy_inventory`.
fn row_count(store: &SqliteStore) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM copy_inventory", [], |row| row.get(0))
                .am("count the copy inventory rows")
        })
        .expect("count the copy inventory rows")
}

/// A database created under the old placeholder shape converges on first use:
/// the table ends as exactly the fresh DDL's eight columns with the unique
/// index, the placeholder row that carries no metadata is gone, and the save
/// lands its own row.
#[test]
fn a_pre_existing_placeholder_table_converges_on_first_use() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    placeholder(&path, "INSERT INTO copy_inventory VALUES ('legacy');");
    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    want(store.save(&row("copy-1")), "save converges and writes");
    assert_eq!(
        store.load("copy-1").expect("load"),
        Some(row("copy-1")),
        "the converged table reads back"
    );
    let repaired = columns(&store);
    assert_eq!(
        repaired,
        expected_columns(),
        "the rebuilt table holds exactly the metadata columns, in DDL order"
    );
    assert!(
        !repaired.iter().any(|name| name == "id"),
        "the legacy key is gone: {repaired:?}"
    );
    let indexes = index_names(&store);
    assert!(
        indexes.iter().any(|name| name == INDEX),
        "the converged table backs {INDEX}: {indexes:?}"
    );
    assert_eq!(
        row_count(&store),
        1,
        "the row that carries no metadata is gone and only the saved row is left"
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
        "a second use leaves the converged columns in place and in order"
    );
    assert!(
        index_names(&reopened).iter().any(|name| name == INDEX),
        "a second use keeps {INDEX}"
    );
}

/// Only a row whose eight metadata columns are all NULL is removed. A row that
/// already holds a metadata value is copied, so the repair never deletes one.
#[test]
fn a_placeholder_row_with_a_metadata_value_is_kept() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    placeholder(
        &path,
        "ALTER TABLE copy_inventory ADD COLUMN copy_id text;
         INSERT INTO copy_inventory (id, copy_id) VALUES ('legacy', 'copy-kept');",
    );
    let store = SqliteStore::open(&path, local_kms()).expect("open an old sqlite database");
    assert_eq!(
        store.load("absent").expect("converge"),
        None,
        "the read converges the table"
    );
    assert_eq!(
        row_count(&store),
        1,
        "the row with a metadata value is kept"
    );
    let kept: Option<String> = store
        .with_conn(|conn| {
            conn.query_row("SELECT copy_id FROM copy_inventory", [], |row| row.get(0))
                .am("read the kept row")
        })
        .expect("read the kept row");
    assert_eq!(
        kept.as_deref(),
        Some("copy-kept"),
        "the copied metadata value round-trips"
    );
    assert_eq!(
        columns(&store),
        expected_columns(),
        "the rebuilt table holds exactly the metadata columns, in DDL order"
    );
}
