// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what every store open path does with
//! the cross-process shared hold: a file-backed `SqliteStore::open` holds it
//! for the store's lifetime and releases it on drop, a second open takes a
//! second hold, the read-only open paths fail closed under a foreign exclusive
//! hold, and the documented exemptions hold.
//!
//! ## Owns
//! - The scratch and foreign-exclusive-holder fixtures these assertions use
//!
//! ## Does not own
//! - The lock primitive under test (see `store_lock`, which has its own suites)
//!
//! ## Boundary
//!
//! Keep this file focused on what an open does with the shared hold.

use super::SqliteStore;
use crate::dek::{DekProvider, LocalDevKms};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-open-lock-{tag}"
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

/// Open a real file-backed store and drop it, so the store and its lock file
/// exist for the read-only paths to act on.
fn create_store(path: &Path) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(path, kms).expect("open a file-backed store");
    drop(store);
}

/// Hold the store's lock file exclusively through a raw descriptor.
///
/// A `flock` contends across descriptors of one process exactly as it does
/// across processes, so this stands in for another process holding the store
/// exclusively during a restore. Dropping the returned `File` releases it.
fn exclusive_holder(path: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(crate::store_lock::lock_file_path(path))
        .expect("open the store's lock file");
    file.lock().expect("hold the store's lock file exclusively");
    file
}

#[test]
fn a_store_open_holds_the_shared_hold_for_its_lifetime_and_releases_on_drop() {
    let scratch = Scratch::new("lifetime");
    let path = scratch.join("store.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&path, kms).expect("open the store");
    assert!(
        crate::store_lock::this_process_holds_shared(&path),
        "an open store must hold the shared hold"
    );
    drop(store);
    assert!(
        !crate::store_lock::this_process_holds_shared(&path),
        "dropping the store must release the shared hold"
    );
}

#[test]
fn a_second_open_takes_a_second_shared_hold_and_both_coexist() {
    let scratch = Scratch::new("coexist");
    let path = scratch.join("store.db");
    let first = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open the first store");
    let second = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open the second store");
    assert!(
        crate::store_lock::this_process_holds_shared(&path),
        "two open stores must hold the lock shared together"
    );
    drop(first);
    assert!(
        crate::store_lock::this_process_holds_shared(&path),
        "dropping one store must leave the second's own shared hold in place"
    );
    drop(second);
    assert!(
        !crate::store_lock::this_process_holds_shared(&path),
        "dropping both stores must release every shared hold"
    );
}

// The three refusals below each take about five seconds on purpose: the
// bounded wait for a contending exclusive hold is the product behavior under
// test, not an accident to be slept away.

#[test]
fn measuring_refuses_while_the_store_is_held_exclusively() {
    let scratch = Scratch::new("state-refusal");
    let path = scratch.join("store.db");
    create_store(&path);
    let holder = exclusive_holder(&path);
    let error = crate::sqlite_file_state(&path).expect_err("the measurement must refuse");
    drop(holder);
    assert!(
        error.message.contains("a restore is replacing it"),
        "unexpected refusal: {}",
        error.message
    );
}

#[test]
fn digesting_refuses_while_the_store_is_held_exclusively() {
    let scratch = Scratch::new("digest-refusal");
    let path = scratch.join("store.db");
    create_store(&path);
    let holder = exclusive_holder(&path);
    let error = crate::sqlite_content_digest(&path).expect_err("the digest must refuse");
    drop(holder);
    assert!(
        error.message.contains("a restore is replacing it"),
        "unexpected refusal: {}",
        error.message
    );
}

#[test]
fn snapshotting_refuses_while_the_store_is_held_exclusively() {
    let scratch = Scratch::new("snapshot-refusal");
    let path = scratch.join("store.db");
    create_store(&path);
    let holder = exclusive_holder(&path);
    let error = crate::snapshot_sqlite_into(&path, &staged_path(&scratch))
        .expect_err("the snapshot must refuse");
    drop(holder);
    assert!(
        error.message.contains("a restore is replacing it"),
        "unexpected refusal: {}",
        error.message
    );
}

#[test]
fn a_refused_snapshot_writes_no_staged_file() {
    let scratch = Scratch::new("snapshot-staged");
    let path = scratch.join("store.db");
    create_store(&path);
    let staged = staged_path(&scratch);
    let holder = exclusive_holder(&path);
    let error = crate::snapshot_sqlite_into(&path, &staged).expect_err("the snapshot must refuse");
    drop(holder);
    assert!(
        error.message.contains("a restore is replacing it"),
        "unexpected refusal: {}",
        error.message
    );
    assert!(
        !staged.exists(),
        "a snapshot refused before the copy must not write the staged file"
    );
}

/// A fresh staged path inside `scratch` that nothing has created yet.
fn staged_path(scratch: &Scratch) -> PathBuf {
    scratch.join(&crate::test_unique::unique("staged"))
}

#[test]
fn measuring_a_missing_target_creates_no_lock_file() {
    let scratch = Scratch::new("missing-target");
    let path = scratch.join("missing.db");
    let state = crate::sqlite_file_state(&path).expect("a missing file is a valid measurement");
    assert!(!state.exists, "the missing target must measure as absent");
    assert!(
        !crate::store_lock::lock_file_path(&path).exists(),
        "the missing-target exemption must not create a lock file"
    );
}

/// A file whose `memtree_nodes` table has the wrong shape and holds rows, so
/// `SqliteStore::open` refuses inside `from_connection` — after the shared
/// hold has already been taken. This is the post-hold error path the
/// release-on-early-failure rule must cover.
fn a_store_that_refuses_after_the_hold(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("create the stand-in file");
    conn.execute_batch(
        "CREATE TABLE memtree_nodes (
           id text PRIMARY KEY,
           bank_id text NOT NULL,
           parent_id text,
           item_id text,
           depth integer NOT NULL DEFAULT 0,
           summary_gist text,
           dirty integer NOT NULL DEFAULT 0,
           created_at text NOT NULL,
           updated_at text NOT NULL
         );
         INSERT INTO memtree_nodes (id, bank_id, depth, dirty, created_at, updated_at)
         VALUES ('n', 'b', 0, 0, 't', 't');",
    )
    .expect("create a stale memtree with rows");
}

/// An open that fails after taking the hold must release it, so a later
/// restore is not blocked by a handle the store never handed out.
#[test]
fn a_store_open_that_fails_after_the_hold_releases_it() {
    let scratch = Scratch::new("post-hold-failure");
    let path = scratch.join("store.db");
    a_store_that_refuses_after_the_hold(&path);
    let Err(error) = SqliteStore::open(&path, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
    else {
        panic!("the stale memtree shape must refuse the open");
    };
    assert_eq!(
        error.code,
        clio_types::ErrorCode::ConfigCorrupt,
        "the refusal is the store's own: {}",
        error.message
    );
    assert!(
        !crate::store_lock::this_process_holds_shared(&path),
        "a failed open must release the shared hold it took"
    );
    // A later restore must not be blocked by the failed open's hold.
    crate::store_lock::ExclusiveHold::acquire_with_wait(
        &path,
        std::time::Duration::from_millis(50),
    )
    .expect("the exclusive hold must be free after the failed open");
}

/// The in-memory target is exempt: it has no file, so it takes no hold and
/// leaves no lock file in the working directory.
#[test]
fn an_in_memory_target_takes_no_hold_and_leaves_no_lock_file() {
    let cwd = std::env::current_dir().expect("the working directory");
    let stray = cwd.join(":memory:.lock");
    let _ = std::fs::remove_file(&stray);
    let store = SqliteStore::open(
        Path::new(":memory:"),
        Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>,
    )
    .expect("an in-memory store must open");
    drop(store);
    assert!(
        !stray.exists(),
        "an in-memory open must not create a lock file in the working directory"
    );
}

#[test]
fn an_unwritable_directory_does_not_break_a_read_only_open_whose_lock_file_exists() {
    let scratch = Scratch::new("unwritable-dir");
    let path = scratch.join("store.db");
    let dir = path.parent().expect("the scratch directory");
    let lock = crate::store_lock::lock_file_path(&path);
    create_store(&path);
    // Making the directory unwritable alone is not enough to exercise the
    // fallback: opening an existing lock file for writing needs no directory
    // permission. Making the lock file itself read-only is what forces the
    // acquire onto the read-only fallback this test pins, so both are set.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555))
        .expect("make the scratch directory unwritable");
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o400))
        .expect("make the lock file read-only");
    // Restored before the assertions so the scratch directory is removable
    // even when the assertions fail.
    let measured = crate::sqlite_file_state(&path);
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600))
        .expect("restore the lock file permissions");
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .expect("restore the scratch directory permissions");
    let state = measured.expect("the read-only open must succeed through the fallback");
    assert!(state.exists, "the measured store must still exist");
}
