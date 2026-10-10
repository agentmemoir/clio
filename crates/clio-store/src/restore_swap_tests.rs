// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the SQLite swap: a restore over a
//! populated store, sidecar removal, the open-handle refusal, a simulated crash
//! before and after the rename, an invalid staged payload, a staged file
//! outside the target's directory, and the key-store outcomes including the
//! partial restore.
//!
//! ## Owns
//! - The scratch-store fixtures and the swap assertions
//!
//! ## Does not own
//! - The pre-flight space check (see `restore_space_tests`)
//! - The archive the payload came from (see `clio-backup`)
//!
//! ## Boundary
//!
//! Keep this file focused on `restore_swap` behavior.

use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use rusqlite::Connection;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-swap-{tag}"
        )))
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A fresh file name inside the scratch directory.
    pub(crate) fn fresh(&self, stem: &str) -> PathBuf {
        self.0.join(crate::test_unique::unique(stem))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file-backed store with `rows` items rows, built through its own opener.
pub(crate) fn populated_store(path: &Path, rows: u32) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
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
///
/// The bytes come from the store's own snapshot path, so the swap is exercised
/// against the payload shape the product produces.
pub(crate) fn staged_payload(dir: &Path, rows: u32) -> PathBuf {
    let source = dir.join("payload-source.db");
    populated_store(&source, rows);
    let staged = dir.join("staged-payload.db");
    crate::snapshot_sqlite_into(&source, &staged).expect("snapshot the payload source");
    let _ = std::fs::remove_file(&source);
    staged
}

/// A staged key-store file holding the SQLite magic.
pub(crate) fn staged_key(dir: &Path) -> PathBuf {
    let path = dir.join("staged-keys.kms.json");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0x5Au8, 64));
    std::fs::write(&path, bytes).expect("write the staged key store");
    path
}

/// A live key store created through the real key provider.
fn live_key_store(path: &Path) {
    // Sealing uses the process test key, so this never reaches the machine's
    // real key store; without it a developer machine holding both a keystore
    // entry and a key file fails every seal with the two-store refusal.
    crate::kms_migrate::install_test_key();
    let kms = LocalDevKms::open_persistent(path).expect("open the key store");
    kms.ensure_dek("subject-0001").expect("write a key slot");
}

/// The sidecar path for one suffix.
pub(crate) fn sidecar(target: &Path, suffix: &str) -> PathBuf {
    let mut os = target.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

#[test]
fn restore_over_a_populated_store_yields_the_archived_counts() {
    let scratch = Scratch::new("counts");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 7);
    let request = SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    };

    let report = swap_sqlite_store(&request).expect("swap");
    assert_eq!(report.count_of("items"), Some(7), "the archive's rows");
    assert_eq!(report.bytes, std::fs::metadata(&target).unwrap().len());
    assert!(!report.is_partial());
    assert!(report.sidecars_removed.is_empty(), "there were no sidecars");
    assert!(!staged.exists(), "the staged file became the target");
    assert!(
        crate::sqlite_integrity_ok(&target).is_ok(),
        "the placed file is a valid database"
    );
}

#[test]
fn sidecars_are_removed_before_the_rename() {
    let scratch = Scratch::new("sidecars");
    let target = scratch.join("clio.db");
    populated_store(&target, 1);
    let wal = sidecar(&target, "-wal");
    let shm = sidecar(&target, "-shm");
    let journal = sidecar(&target, "-journal");
    for path in [&wal, &shm, &journal] {
        std::fs::write(path, b"stale sidecar bytes").expect("seed a sidecar");
    }
    let staged = staged_payload(&scratch.0, 2);

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect("swap");

    assert_eq!(report.sidecars_removed.len(), 3);
    assert!(report.sidecars_removed.contains(&wal));
    assert!(report.sidecars_removed.contains(&shm));
    assert!(report.sidecars_removed.contains(&journal));
    for path in [&wal, &shm, &journal] {
        assert!(!path.exists(), "{} is gone", path.display());
    }
    assert_eq!(report.count_of("items"), Some(2));
}

#[test]
fn an_open_store_handle_refuses_the_swap_and_leaves_the_sidecars_alone() {
    let scratch = Scratch::new("open-handle");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 9);

    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let held = SqliteStore::open(&target, kms).expect("hold the target open");
    let before = std::fs::read(&target).expect("read the target");
    // Seeded after the store is open: a stray `-wal` makes SQLite open the
    // file in WAL mode and rewrite its header, which has nothing to do with us.
    let wal = sidecar(&target, "-wal");
    std::fs::write(&wal, b"stale sidecar bytes").expect("seed a sidecar");
    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("an open handle must refuse the swap");
    assert!(
        error.message.contains("open store handle"),
        "the refusal is actionable: {}",
        error.message
    );
    drop(held);

    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(wal.exists(), "no sidecar was touched");
    assert!(staged.exists(), "the staged payload was not consumed");
    assert_eq!(
        crate::sqlite_file_state(&target).unwrap().count_of("items"),
        Some(3),
        "the old store is intact"
    );
}

#[test]
fn a_stop_before_the_rename_leaves_the_old_database_intact() {
    let scratch = Scratch::new("stop-before");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 8);
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_with_stop(
        &SwapRequest {
            staged_payload: staged.clone(),
            target: target.clone(),
            key: None,
        },
        FaultPoint::AfterSidecars,
    )
    .expect_err("the simulated crash stops the swap");
    assert!(
        error.message.contains("simulated crash"),
        "{}",
        error.message
    );

    assert_eq!(
        std::fs::read(&target).expect("read the target"),
        before,
        "the target is byte-identical"
    );
    assert!(
        crate::sqlite_integrity_ok(&target).is_ok(),
        "the old database is still readable"
    );
    assert!(staged.exists(), "the staged payload is still staged");
}

#[test]
fn a_stop_after_the_rename_leaves_the_new_database_in_place() {
    let scratch = Scratch::new("stop-after");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 8);

    let error = swap_with_stop(
        &SwapRequest {
            staged_payload: staged.clone(),
            target: target.clone(),
            key: None,
        },
        FaultPoint::AfterRename,
    )
    .expect_err("the simulated crash stops the swap");
    assert!(
        error.message.contains("simulated crash"),
        "{}",
        error.message
    );

    assert!(!staged.exists());
    assert_eq!(
        crate::sqlite_file_state(&target).unwrap().count_of("items"),
        Some(8),
        "the target is the new database, never a mix"
    );
}

#[test]
fn a_staged_payload_that_is_not_a_database_aborts_before_anything_is_replaced() {
    let scratch = Scratch::new("junk-payload");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let wal = sidecar(&target, "-wal");
    std::fs::write(&wal, b"stale sidecar bytes").expect("seed a sidecar");
    let staged = scratch.fresh("junk-payload.db");
    std::fs::write(&staged, b"this is not a SQLite database").expect("seed the staged file");
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("a payload that is not a database must abort");
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{}", error.message);

    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(wal.exists(), "no sidecar was touched");
    assert!(staged.exists());
}

#[test]
fn a_staged_payload_outside_the_target_directory_is_refused() {
    let scratch = Scratch::new("elsewhere");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let other = scratch.join("other");
    std::fs::create_dir_all(&other).expect("create the other directory");
    let staged = staged_payload(&other, 5);
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect_err("a staged file elsewhere must be refused");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("not in the target's directory"),
        "{}",
        error.message
    );
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
}

#[test]
fn a_key_swap_failure_is_reported_as_partial_with_the_database_replaced() {
    let scratch = Scratch::new("partial");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 6);
    let staged_keys = staged_key(&scratch.0);
    // A directory where the key file belongs: the rename cannot succeed.
    let destination = scratch.join("clio.db.kms.json");
    std::fs::create_dir_all(&destination).expect("create the blocking directory");

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: staged_keys.clone(),
            destination: destination.clone(),
        }),
    })
    .expect("the database swap itself succeeds");

    assert!(report.is_partial(), "the key was not replaced");
    assert!(!report.key_replaced);
    let failure = report.failure.as_deref().expect("a failure is reported");
    assert_eq!(
        report.failed_step,
        Some(FAILED_STEP_KEY),
        "the step is named"
    );
    assert!(
        failure.contains(&destination.display().to_string()),
        "the report names the key store: {failure}"
    );
    assert_eq!(
        crate::sqlite_file_state(&target).unwrap().count_of("items"),
        Some(6),
        "the database is the new one"
    );
    assert!(staged_keys.exists(), "the staged key was not consumed");
}

#[test]
fn a_key_swap_replaces_the_live_key_store() {
    let scratch = Scratch::new("keys");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 5);
    let staged_keys = staged_key(&scratch.0);
    let expected = std::fs::read(&staged_keys).expect("read the staged key store");
    let destination = scratch.join("clio.db.kms.json");

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: staged_keys.clone(),
            destination: destination.clone(),
        }),
    })
    .expect("swap");

    assert!(report.key_replaced);
    assert!(!report.is_partial());
    assert_eq!(
        std::fs::read(&destination).expect("read the replaced key store"),
        expected,
        "the key store is the archive's bytes"
    );
    assert!(!staged_keys.exists(), "the staged key was renamed");
}

#[test]
fn an_open_key_store_handle_refuses_the_swap() {
    let scratch = Scratch::new("open-keys");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let destination = scratch.join("clio.db.kms.json");
    live_key_store(&destination);
    let staged = staged_payload(&scratch.0, 5);
    let staged_keys = staged_key(&scratch.0);
    let before = std::fs::read(&target).expect("read the target");

    let held = LocalDevKms::open_persistent(&destination).expect("hold the key store open");
    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: staged_keys.clone(),
            destination: destination.clone(),
        }),
    })
    .expect_err("an open key store must refuse the swap");
    drop(held);

    assert!(
        error.message.contains("open store handle"),
        "the refusal is actionable: {}",
        error.message
    );
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(staged.exists(), "the staged payload was not consumed");
    assert!(staged_keys.exists(), "the staged key was not consumed");
}

#[test]
fn a_staged_key_that_is_not_a_key_store_is_refused_before_anything_changes() {
    let scratch = Scratch::new("junk-keys");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 5);
    let staged_keys = scratch.fresh("junk-keys.kms.json");
    std::fs::write(&staged_keys, b"this is not a key store").expect("seed the staged key");
    let destination = scratch.join("clio.db.kms.json");
    let before = std::fs::read(&target).expect("read the target");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: Some(KeyPlacement {
            staged: staged_keys.clone(),
            destination,
        }),
    })
    .expect_err("a staged key that is not a key store must be refused");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(staged.exists(), "nothing was consumed");
    assert!(staged_keys.exists());
}
