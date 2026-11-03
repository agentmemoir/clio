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

use super::{STORE_BUSY_TIMEOUT, SqliteStore};
use crate::dek::{DekProvider, LocalDevKms};
use clio_types::{AmError, ErrorCode};
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
    // FULL and delete are the compiled defaults of the bundled SQLite, so these
    // two assertions stay green even when the open never sets them. They guard
    // against a future change to another mode; they do not prove the statements
    // ran.
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
fn a_read_only_handle_opened_like_the_reads_waits_ten_seconds() {
    let scratch = Scratch::new("read-only");
    let path = scratch.join("store.db");
    let store = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open a file-backed store");
    // The digest, the snapshot, the restore measurement, and the setup count
    // each open the live store this way: read-only, then the store's wait.
    let reader =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the store read-only");
    reader
        .busy_timeout(STORE_BUSY_TIMEOUT)
        .expect("apply the store wait to the read-only handle");
    let waited: i64 = reader
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("read the busy timeout through the read-only handle");
    assert_eq!(
        waited, 10000,
        "a read-only open must wait ten seconds instead of failing at once"
    );
    drop(store);
}
