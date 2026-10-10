// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the open-handle registry: the count
//! follows a handle's lifetime, paths do not share counts, a path that does not
//! exist yet keys the same as one that does, and the store and key-store
//! openers register and release their paths.
//!
//! ## Owns
//! - The scratch-path fixture and the registry assertions
//!
//! ## Does not own
//! - The swap that consults the count (see `restore_swap_tests`)
//! - Store behavior (see the store suites)
//!
//! ## Boundary
//!
//! Keep this file focused on `open_handles` behavior.

use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-open-handles-{tag}"
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

/// A file that exists, so the registry has a real path to key.
fn seed_file(scratch: &Scratch) -> PathBuf {
    let path = scratch.join("clio.db");
    std::fs::write(&path, b"not a database").expect("seed the file");
    path
}

#[test]
fn a_registration_counts_and_releases_with_the_handle() {
    let scratch = Scratch::new("count");
    let path = seed_file(&scratch);
    assert_eq!(open_handles(&path), 0, "nothing registered yet");

    let first = register(&path);
    assert_eq!(open_handles(&path), 1);
    let second = register(&path);
    assert_eq!(open_handles(&path), 2, "two handles on one path");
    drop(first);
    assert_eq!(open_handles(&path), 1, "one handle still open");
    drop(second);
    assert_eq!(open_handles(&path), 0, "the count follows the handles");
}

#[test]
fn two_paths_do_not_share_a_count() {
    let scratch = Scratch::new("separate");
    let first = seed_file(&scratch);
    let second = scratch.join("other.db");
    std::fs::write(&second, b"x").expect("seed the second file");

    let held = register(&first);
    assert_eq!(open_handles(&first), 1);
    assert_eq!(open_handles(&second), 0, "another path is not affected");
    assert_eq!(open_handles(&scratch.join("missing.db")), 0);
    drop(held);
}

#[test]
fn a_path_that_does_not_exist_yet_keys_like_the_created_file() {
    let scratch = Scratch::new("not-yet");
    let path = scratch.join("clio.db");
    assert_eq!(open_handles(&path), 0);

    let held = register(&path);
    std::fs::write(&path, b"created after registration").expect("create the file");
    assert_eq!(
        open_handles(&path),
        1,
        "creating the file must not change the key"
    );
    drop(held);
    assert_eq!(open_handles(&path), 0);
}

#[test]
fn a_file_backed_store_registers_and_releases_its_path() {
    let scratch = Scratch::new("store");
    let path = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());

    let store = SqliteStore::open(&path, Arc::clone(&kms)).expect("open the store");
    assert_eq!(open_handles(&path), 1, "the open store is counted");
    let second = SqliteStore::open(&path, kms).expect("open it again");
    assert_eq!(open_handles(&path), 2, "both handles are counted");
    drop(store);
    assert_eq!(open_handles(&path), 1);
    drop(second);
    assert_eq!(open_handles(&path), 0);
}

#[test]
fn an_in_memory_store_registers_nothing() {
    let scratch = Scratch::new("memory");
    let path = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("open in memory");
    assert_eq!(open_handles(&path), 0, "there is no file to key");
    drop(store);
}

#[test]
fn a_file_backed_key_store_registers_and_releases_its_path() {
    let scratch = Scratch::new("keys");
    let path = scratch.join("clio.kms.json");
    let kms = LocalDevKms::open_persistent(&path).expect("open the key store");
    assert_eq!(open_handles(&path), 1, "the key store is counted");
    drop(kms);
    assert_eq!(open_handles(&path), 0);
}
