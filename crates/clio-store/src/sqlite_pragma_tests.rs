// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the connection pragmas every
//! `SqliteStore` open applies: a contended open waits instead of failing at
//! once, the sync mode is stated rather than implied, and the journal stays in
//! rollback mode so no `-wal` or `-shm` sidecar appears beside the database.
//!
//! ## Owns
//! - The scratch fixture these assertions open a file-backed store in
//!
//! ## Does not own
//! - The pragma values themselves (see `from_connection` in `sqlite`)
//! - The key store's own pragmas (see `local_kms_schema`)
//!
//! ## Boundary
//!
//! Keep this file focused on what an open sets on the connection.

use super::SqliteStore;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::sqlite_history_tests::item;
use crate::store::Store;
use clio_types::{AmError, EpisodicType, ErrorCode};
use std::path::PathBuf;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-pragma-{tag}"
        )))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read one integer-valued pragma through the store's own connection.
fn pragma_int(store: &SqliteStore, sql: &str) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row(sql, [], |r| r.get::<_, i64>(0))
                .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .expect("read an integer pragma")
}

/// Read one text-valued pragma through the store's own connection.
fn pragma_text(store: &SqliteStore, sql: &str) -> String {
    store
        .with_conn(|conn| {
            conn.query_row(sql, [], |r| r.get::<_, String>(0))
                .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .expect("read a text pragma")
}

#[test]
fn a_file_backed_open_applies_the_store_pragmas() {
    let scratch = Scratch::new("applied");
    let path = scratch.join("store.db");
    let store = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open a file-backed store");
    assert_eq!(
        pragma_int(&store, "PRAGMA busy_timeout"),
        10000,
        "a contended open must wait ten seconds instead of failing at once"
    );
    assert_eq!(
        pragma_int(&store, "PRAGMA synchronous"),
        2,
        "the open must state FULL sync rather than inherit the default"
    );
    assert_eq!(
        pragma_text(&store, "PRAGMA journal_mode"),
        "delete",
        "the store must stay in rollback mode, so no -wal or -shm sidecar appears"
    );
}

#[test]
fn a_file_backed_open_still_writes_and_reads_back_an_item() {
    let scratch = Scratch::new("write");
    let path = scratch.join("store.db");
    let store = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open a file-backed store");
    let bank = crate::test_unique::unique("pragma-bank");
    let item_id = crate::test_unique::unique("pragma-item");
    let row = item(&item_id, &bank, EpisodicType::Task, false);
    store
        .create_memory_item(&row, "agent")
        .expect("write one item after the pragma change");
    let read = store
        .get_memory_item_impl(&bank, &item_id)
        .expect("read the item back")
        .expect("the written item exists");
    assert_eq!(read.id, item_id);
    assert_eq!(read.bank_id, bank);
}
