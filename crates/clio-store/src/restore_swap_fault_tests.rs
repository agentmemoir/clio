// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the swap's failure arms: a key
//! member staged outside the target's directory, a sidecar that cannot be
//! removed, a rename that fails, and the error mappers every path shares.
//!
//! ## Owns
//! - The deliberately failed paths and their assertions
//!
//! ## Does not own
//! - The happy paths and the refusals (see `restore_swap_tests`)
//! - The pre-flight space check (see `restore_space_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on `restore_swap`'s failure arms.

use super::*;
use crate::dek::DekProvider;
use crate::sqlite::SqliteStore;
use rusqlite::Connection;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-fault-{tag}"
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

/// A file-backed store with `rows` items rows.
fn populated_store(path: &Path, rows: u32) {
    let kms: Arc<dyn DekProvider> = Arc::new(crate::dek::LocalDevKms::new());
    let store = SqliteStore::open(path, kms).expect("open a file-backed store");
    drop(store);
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

/// A real staged payload holding `rows` items, placed in `dir`.
fn staged_payload(dir: &Path, rows: u32) -> PathBuf {
    let source = dir.join("fault-source.db");
    populated_store(&source, rows);
    let staged = dir.join("fault-staged.db");
    crate::snapshot_sqlite_into(&source, &staged).expect("snapshot the payload source");
    let _ = std::fs::remove_file(&source);
    staged
}

/// The sidecar path for one suffix.
fn sidecar(target: &Path, suffix: &str) -> PathBuf {
    let mut os = target.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// A staged key store holding the SQLite magic.
fn staged_key(dir: &Path) -> PathBuf {
    let path = dir.join("fault-staged-keys.kms.json");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0x5Au8, 32));
    std::fs::write(&path, bytes).expect("write the staged key store");
    path
}

/// A staged payload that is a real database but whose `items` table has no
/// `bank_id` column, so the store's own shape check fails when the swap
/// measures the store it just placed. The integrity check still passes, which
/// is the point: the payload is valid, it just cannot be counted.
fn uncountable_payload(dir: &Path) -> PathBuf {
    let path = dir.join("fault-uncountable.db");
    let conn = Connection::open(&path).expect("create the staged payload");
    conn.execute_batch(
        "CREATE TABLE items (id TEXT PRIMARY KEY, body TEXT);
         INSERT INTO items (id, body) VALUES ('a', 'b');",
    )
    .expect("build the odd store");
    drop(conn);
    path
}

/// A hot journal beside the key store, as a key store killed mid-transaction
/// leaves behind.
fn hot_journal_beside(key_store: &Path) -> PathBuf {
    let journal = sidecar(key_store, "-journal");
    std::fs::write(&journal, b"a hot rollback journal").expect("seed the journal");
    journal
}

#[test]
fn the_key_stores_own_journal_is_removed_before_it_is_replaced() {
    let scratch = Scratch::new("key-journal");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 5);
    let staged_keys = staged_key(&scratch.0);
    let destination = scratch.join("clio.db.kms.json");
    let journal = hot_journal_beside(&destination);

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: staged_keys,
            destination: destination.clone(),
        }),
    })
    .expect("the swap completes");

    assert!(report.key_replaced, "the key store was replaced");
    // SQLite resolves a journal by name and replays it against whatever holds
    // that name, so a journal left beside the new key store would roll the old
    // key store's pages into the archive's bytes.
    assert!(
        !journal.exists(),
        "the old key store's journal is gone before the rename"
    );
}

#[test]
fn a_store_that_cannot_be_measured_after_the_rename_is_reported_not_thrown() {
    let scratch = Scratch::new("unmeasured");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = uncountable_payload(&scratch.0);

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect("a measurement failure after the rename is not a returned error");

    // The store really was replaced, so the caller has to be able to say so
    // rather than losing the report — and with it the pre-restore snapshot id,
    // which is the operator's only way back.
    assert!(report.is_partial(), "the run did not fully complete");
    assert_eq!(report.failed_step, Some(FAILED_STEP_MEASURE), "{report:?}");
    let failure = report.failure.as_deref().expect("a failure is reported");
    assert!(
        failure.contains("was replaced but could not be measured"),
        "{failure}"
    );
    assert!(
        report.table_counts.is_empty() && report.banks.is_empty(),
        "nothing was counted, and nothing pretends otherwise: {report:?}"
    );
    assert!(
        std::fs::read(&target)
            .expect("the target is the new file")
            .starts_with(b"SQLite format 3\0"),
        "the database was replaced"
    );
}

#[test]
fn a_key_member_staged_outside_the_target_directory_is_refused() {
    let scratch = Scratch::new("key-elsewhere");
    let target = scratch.join("clio.db");
    populated_store(&target, 2);
    let staged = staged_payload(&scratch.0, 3);
    let other = scratch.join("other");
    std::fs::create_dir_all(&other).expect("create the other directory");
    let keys = staged_key(&other);
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: keys.clone(),
            destination: scratch.join("clio.db.kms.json"),
        }),
    })
    .expect_err("a key staged elsewhere must be refused");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("not in the target's directory"),
        "{}",
        error.message
    );
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(staged.exists() && keys.exists(), "nothing was consumed");
}

#[test]
fn a_sidecar_that_cannot_be_removed_aborts_before_the_rename() {
    let scratch = Scratch::new("sidecar-dir");
    let target = scratch.join("clio.db");
    populated_store(&target, 2);
    // A directory where the write-ahead log belongs: remove_file cannot remove it.
    std::fs::create_dir(sidecar(&target, "-wal")).expect("create the blocking sidecar");
    let staged = staged_payload(&scratch.0, 4);
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("a sidecar that cannot be removed must abort the swap");
    assert!(
        error.message.contains("-wal"),
        "the refusal names the sidecar: {}",
        error.message
    );
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(staged.exists(), "the staged payload is still staged");
}

#[test]
fn a_rename_that_fails_reports_the_target_and_keeps_the_staged_payload() {
    let scratch = Scratch::new("rename-fails");
    // A non-empty directory where the store file belongs: the rename cannot
    // replace it, and the target is not a database the swap may touch.
    let target = scratch.join("clio.db");
    std::fs::create_dir_all(&target).expect("create the blocking target");
    std::fs::write(target.join("occupied"), b"x").expect("make it non-empty");
    let staged = staged_payload(&scratch.0, 4);

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("a rename onto a non-empty directory must fail");
    assert!(
        error.message.contains("placing the staged payload"),
        "the failure names the step: {}",
        error.message
    );
    assert!(target.is_dir(), "the blocking target is untouched");
    assert!(staged.exists(), "the staged payload is still staged");
}

#[test]
fn the_error_mappers_cover_their_failure_arms() {
    let scratch = Scratch::new("mappers");
    assert!(file_bytes(&scratch.join("missing.db")).is_err());
    assert!(
        canonical_parent(Path::new("clio.db")).is_err(),
        "a bare name has no directory"
    );
    assert_eq!(
        canonical_parent(&scratch.join("missing-dir").join("clio.db")).expect("a parent"),
        scratch.join("missing-dir"),
        "a parent that cannot be canonicalized is used as given"
    );
    assert!(
        !sync_parent(Path::new("clio.db")),
        "a bare name has no directory to fsync"
    );
    assert!(
        !sync_parent(&scratch.join("missing-dir").join("clio.db")),
        "a directory that cannot be opened is not synced"
    );
    assert!(
        refuse_not_a_key_store(&scratch.0).is_err(),
        "a directory cannot be read as a key store"
    );
    assert!(
        refuse_not_a_key_store(&scratch.join("missing-key")).is_err(),
        "a missing file cannot be opened"
    );
}
