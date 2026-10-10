// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the read-only store measurement: a
//! missing file measures as absent rather than failing, a populated store
//! reports every user table, a bootstrapped store is not empty, and a path that
//! cannot be read as a database is an error rather than a false zero.
//!
//! ## Owns
//! - The scratch-store fixture and the measurement assertions
//!
//! ## Does not own
//! - The swap that uses the measurement (see `restore_swap_tests`)
//! - The snapshot copy itself (see `sqlite_snapshot_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on `restore_state` behavior.

use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use rusqlite::Connection;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-state-{tag}"
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

/// A file-backed store with `rows` items rows, built through its own opener.
fn populated_store(path: &Path, rows: u32) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open(path, kms).expect("open a file-backed store");
    let conn = Connection::open(path).expect("open the store file");
    for index in 0..rows {
        conn.execute(
            "INSERT INTO items (id, bank_id, kind, epistemic_kind, content_ciphertext,
                                valid_from, tx_from, created_at, updated_at)
             VALUES (?1, 'bank-a', 'episodic', 'fact', 'ciphertext', 't0', 't0', 't0', 't0')",
            [format!("item-{index:04}")],
        )
        .expect("insert a row");
    }
}

#[test]
fn a_missing_file_measures_as_absent_and_empty() {
    let scratch = Scratch::new("missing");
    let state = sqlite_file_state(&scratch.join("clio.db")).expect("measure a missing file");
    assert!(!state.exists);
    assert_eq!(state.bytes, 0);
    assert_eq!(state.table_count(), 0);
    assert!(state.banks.is_empty(), "no file, no banks");
    assert!(state.is_empty(), "nothing is there, so nothing is at risk");
}

#[test]
fn a_populated_store_reports_its_tables_and_is_not_empty() {
    let scratch = Scratch::new("populated");
    let path = scratch.join("clio.db");
    populated_store(&path, 5);

    let state = sqlite_file_state(&path).expect("measure the store");
    assert!(state.exists);
    assert!(state.bytes > 0, "the file has bytes");
    assert_eq!(state.count_of("items"), Some(5));
    assert_eq!(state.banks, vec!["bank-a".to_owned()], "{:?}", state.banks);
    assert!(
        state.table_count() > 1,
        "every user table is counted: {}",
        state.table_count()
    );
    assert!(!state.is_empty());
}

#[test]
fn a_fresh_store_is_not_empty_because_opening_it_bootstraps_rows() {
    let scratch = Scratch::new("fresh");
    let path = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&path, kms).expect("open a fresh store");
    drop(store);

    let state = sqlite_file_state(&path).expect("measure the fresh store");
    assert!(state.exists);
    assert!(
        state.banks.is_empty(),
        "settings rows are not banks: {:?}",
        state.banks
    );
    // Opening a store writes its settings rows, so it is not empty. Calling it
    // empty is what let a restore skip the snapshot for a store holding real
    // state in the tables the two headline counts never look at.
    assert!(
        !state.is_empty(),
        "a bootstrapped store is not empty: {:?}",
        state.table_counts
    );
}

#[test]
fn only_a_store_with_no_rows_in_any_table_is_empty() {
    let bare = SqliteFileState {
        path: PathBuf::from("/nowhere/clio.db"),
        exists: true,
        bytes: 4096,
        table_counts: vec![
            ("items".to_owned(), 0),
            ("triples".to_owned(), 0),
            ("beliefs".to_owned(), 0),
        ],
        banks: Vec::new(),
    };
    assert!(
        bare.is_empty(),
        "no table holds a row: {:?}",
        bare.table_counts
    );

    // The case that used to be decided the other way: no memories, but real
    // state elsewhere. A restore replaces the whole file, so these rows are
    // something to preserve and the snapshot must not be skipped.
    let no_memories = SqliteFileState {
        table_counts: vec![
            ("items".to_owned(), 0),
            ("triples".to_owned(), 0),
            ("sync_cursors".to_owned(), 4),
        ],
        ..bare
    };
    assert!(
        !no_memories.is_empty(),
        "rows outside items and triples are still something to preserve: {:?}",
        no_memories.table_counts
    );
}

#[test]
fn a_directory_is_refused_rather_than_measured() {
    let scratch = Scratch::new("directory");
    let error = sqlite_file_state(&scratch.0).expect_err("a directory is not a store file");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("not a regular file"),
        "{}",
        error.message
    );
}

#[test]
fn a_file_that_is_not_a_database_is_an_error_not_an_empty_measurement() {
    let scratch = Scratch::new("not-a-db");
    let path = scratch.join("clio.db");
    std::fs::write(&path, b"this is not a SQLite database").expect("seed the file");

    let error = sqlite_file_state(&path).expect_err("a non-database must not measure as empty");
    assert!(
        !error.message.is_empty(),
        "the refusal says what could not be read"
    );
}

#[test]
fn a_path_that_cannot_be_examined_is_an_error_not_an_absent_file() {
    let scratch = Scratch::new("unreadable-path");
    // A regular file where a directory belongs: the path below it cannot be
    // examined at all, which is not the same as the file being absent.
    let blocker = scratch.join("blocker");
    std::fs::write(&blocker, b"x").expect("seed the blocking file");

    let error = sqlite_file_state(&blocker.join("clio.db"))
        .expect_err("a path under a file cannot be measured");
    assert!(
        error.message.contains("reading the store file"),
        "the refusal names the step: {}",
        error.message
    );
}

#[test]
fn a_snapshot_copy_measures_like_the_store_it_came_from() {
    let scratch = Scratch::new("copy");
    let source = scratch.join("source.db");
    populated_store(&source, 4);
    let staged = scratch.join("staged.db");

    let facts = crate::snapshot_sqlite_into(&source, &staged).expect("snapshot the source");
    let state = sqlite_file_state(&staged).expect("measure the copy");
    assert_eq!(state.count_of("items"), Some(4));
    assert_eq!(facts.count_of("items"), Some(4));
    assert!(!state.is_empty());
}
