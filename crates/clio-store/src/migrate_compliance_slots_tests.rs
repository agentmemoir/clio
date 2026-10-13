// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that both store open paths load the local-compliance slot
//! schema and that repeated loads keep the schema inventory stable: a fresh
//! SQLite store and a fresh loopback Postgres store expose every reserved slot
//! table, and reopening the same store or creating a second fresh store yields
//! the same table inventory.
//!
//! ## Owns
//! - The reserved slot-table list these assertions pin
//! - The scratch SQLite directory and scratch Postgres database fixtures
//!
//! ## Does not own
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - The open-path bootstrap order (`sqlite` / `postgres`)
//! - Shared-Postgres test serialization (`pg_lock`)
//!
//! ## Boundary
//!
//! Tests here break when the slot batch fails to load or stops being
//! idempotent. Feature behavior that later fills a slot belongs to the owning
//! group's own suites.
//!
//! Test-only. Never compiled into production builds.

use crate::PostgresStore;
use crate::SqliteStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;

/// The reserved slot tables, sorted to match the inventory assertions.
const SLOT_TABLES: [&str; 8] = [
    "archive_experiments",
    "copy_inventory",
    "durable_requests",
    "erasure_adapters",
    "local_erasure_journal",
    "local_events",
    "restore_guards",
    "subject_family_links",
];

/// Every slot table the static asset declares matches the pinned list.
#[test]
fn the_slot_asset_declares_every_reserved_table() {
    for slot in SLOT_TABLES {
        assert!(
            super::SQL_COMPLIANCE_SLOTS.contains(&format!("CREATE TABLE IF NOT EXISTS {slot}")),
            "slot asset is missing the reserved table {slot}"
        );
    }
}

/// Require every reserved slot table in a sorted table inventory.
fn assert_slots_present(inventory: &[String]) {
    for slot in SLOT_TABLES {
        assert!(
            inventory.iter().any(|name| name == slot),
            "slot table {slot} missing; inventory: {inventory:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

/// A scratch directory removed when the guard drops (the `sqlite_open_lock_tests`
/// fixture shape).
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir(
            "clio-store-compliance-slots",
        ))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The table inventory of a SQLite store, sorted in Rust so no collation order
/// is assumed.
fn sqlite_inventory(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                )
                .am("slot inventory read")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .am("slot inventory read")?;
            let mut names = rows
                .collect::<Result<Vec<_>, _>>()
                .am("slot inventory read")?;
            names.sort();
            Ok(names)
        })
        .expect("read the sqlite table inventory")
}

#[test]
fn a_fresh_sqlite_store_loads_every_compliance_slot() {
    let scratch = Scratch::new();
    let store = SqliteStore::open(&scratch.0.join("store.db"), local_kms())
        .expect("open a fresh sqlite store");
    let inventory = sqlite_inventory(&store);
    assert_slots_present(&inventory);
}

#[test]
fn repeated_sqlite_loads_keep_the_schema_inventory_stable() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let first = SqliteStore::open(&path, local_kms()).expect("first open");
    let baseline = sqlite_inventory(&first);
    drop(first);
    // Reopen the same database: the slot batch runs again and must not
    // duplicate or change any schema state.
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    assert_eq!(sqlite_inventory(&reopened), baseline);
    drop(reopened);
    // A second fresh store must land on the same inventory.
    let second_scratch = Scratch::new();
    let second = SqliteStore::open(&second_scratch.0.join("store.db"), local_kms())
        .expect("second fresh store");
    assert_eq!(sqlite_inventory(&second), baseline);
}

// ---------------------------------------------------------------------------
// Postgres
// ---------------------------------------------------------------------------

/// The shared Postgres URL, or `None` when the database is unreachable (the
/// `postgres_bootstrap_lock_tests` fixture shape).
fn shared_url() -> Option<String> {
    let url = postgres_url_from_env();
    crate::pg_tls::connect(&url).ok().map(|_| url)
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
        let name = format!("clio_slots_{label}_{}_{nanos}", std::process::id());
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

/// Point a Postgres URL at a different database on the same server, keeping
/// any `?options` suffix. No shared helper exists (`db_url` only resolves the
/// shared URL; `pg_url` only reads SSL options), so the surgery stays here.
fn with_database(base: &str, name: &str) -> String {
    let (path, query) = base
        .split_once('?')
        .map_or((base, None), |(p, q)| (p, Some(q)));
    let server = path.rsplit_once('/').map_or(path, |(s, _)| s);
    match query {
        Some(q) => format!("{server}/{name}?{q}"),
        None => format!("{server}/{name}"),
    }
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    with_database(&postgres_url_from_env(), name)
}

/// A query string on the shared URL must survive the scratch-database swap.
#[test]
fn scratch_url_keeps_a_query_string() {
    assert_eq!(
        with_database("postgres://u:p@h:5432/clio?sslmode=disable", "scratch"),
        "postgres://u:p@h:5432/scratch?sslmode=disable"
    );
}

/// A plain URL with no options keeps its existing shape.
#[test]
fn scratch_url_without_options_is_unchanged_in_shape() {
    assert_eq!(
        with_database("postgres://u:p@h:5432/clio", "scratch"),
        "postgres://u:p@h:5432/scratch"
    );
}

/// Several options survive together, in order.
#[test]
fn scratch_url_keeps_several_options() {
    assert_eq!(
        with_database(
            "postgres://u:p@h:5432/clio?sslmode=require&connect_timeout=10",
            "scratch"
        ),
        "postgres://u:p@h:5432/scratch?sslmode=require&connect_timeout=10"
    );
}

/// The base-table inventory of a Postgres store's public schema, sorted in
/// Rust so no collation order is assumed.
fn postgres_inventory(store: &PostgresStore) -> Vec<String> {
    store
        .with_client(|client| {
            let rows = client
                .query(
                    "SELECT table_name FROM information_schema.tables
                     WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
                    &[],
                )
                .am("slot inventory read")?;
            let mut names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
            names.sort();
            Ok(names)
        })
        .expect("read the postgres table inventory")
}

#[test]
fn a_fresh_postgres_store_loads_every_compliance_slot() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "fresh");
    drop(admin);
    let store = PostgresStore::open(&url_for_database(&db.name), local_kms())
        .expect("open a fresh postgres store");
    let inventory = postgres_inventory(&store);
    assert_slots_present(&inventory);
}

#[test]
fn repeated_postgres_loads_keep_the_schema_inventory_stable() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "stable");
    drop(admin);
    let db_url = url_for_database(&db.name);
    let first = PostgresStore::open(&db_url, local_kms()).expect("first open");
    let baseline = postgres_inventory(&first);
    drop(first);
    // Reopen the same database: the slot batch runs again and must not
    // duplicate or change any schema state.
    let reopened = PostgresStore::open(&db_url, local_kms()).expect("reopen");
    assert_eq!(postgres_inventory(&reopened), baseline);
    drop(reopened);
    // A second fresh database must land on the same inventory.
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let second_db = ScratchDatabase::create(&mut admin, "stable2");
    drop(admin);
    let second = PostgresStore::open(&url_for_database(&second_db.name), local_kms())
        .expect("second fresh store");
    assert_eq!(postgres_inventory(&second), baseline);
}

// ---------------------------------------------------------------------------
// Placeholder column sets
// ---------------------------------------------------------------------------

/// The column names of one SQLite table, in `cid` order.
fn sqlite_columns(store: &SqliteStore, table: &str) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
                ))
                .am("slot column read")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .am("slot column read")?;
            rows.collect::<Result<Vec<_>, _>>().am("slot column read")
        })
        .expect("read the sqlite column set")
}

/// The column names of one Postgres table, in ordinal order.
fn postgres_columns(store: &PostgresStore, table: &str) -> Vec<String> {
    store
        .with_client(|client| {
            let rows = client
                .query(
                    "SELECT column_name FROM information_schema.columns
                     WHERE table_schema = 'public' AND table_name = $1
                     ORDER BY ordinal_position",
                    &[&table],
                )
                .am("slot column read")?;
            Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
        })
        .expect("read the postgres column set")
}

/// Every slot table still holds only the placeholder key: a later phase that
/// adds a feature column must update this pin, not slip past it.
#[test]
fn slot_tables_hold_only_the_placeholder_key() {
    let scratch = Scratch::new();
    let store = SqliteStore::open(&scratch.0.join("store.db"), local_kms())
        .expect("open a fresh sqlite store");
    for slot in SLOT_TABLES {
        assert_eq!(
            sqlite_columns(&store, slot),
            vec!["id".to_owned()],
            "slot table {slot} gained a column"
        );
    }
}

/// The Postgres slot tables hold the same placeholder-only column set.
#[test]
fn postgres_slot_tables_hold_only_the_placeholder_key() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "columns");
    drop(admin);
    let store = PostgresStore::open(&url_for_database(&db.name), local_kms())
        .expect("open a fresh postgres store");
    for slot in SLOT_TABLES {
        assert_eq!(
            postgres_columns(&store, slot),
            vec!["id".to_owned()],
            "slot table {slot} gained a column"
        );
    }
}
