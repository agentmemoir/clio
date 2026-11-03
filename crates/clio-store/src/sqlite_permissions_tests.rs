// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the permissions a file-backed open
//! leaves on the store file: a database this open creates and a database an
//! earlier build created are both owner-only afterwards.
//!
//! ## Owns
//! - The scratch directory these assertions use
//! - Reading and setting the store file's permission bits
//!
//! ## Does not own
//! - The permissions helper under test (see `sqlite_path`)
//! - What an open does with the shared hold (see `sqlite_open_lock_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the mode of the store file. Test-only. Never
//! compiled into production builds.

use super::SqliteStore;
use crate::dek::{DekProvider, LocalDevKms};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-permissions-{tag}"
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

/// The key provider a file-backed open needs; nothing here seals a value.
fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

/// Assert the store file is owner-only.
#[cfg(unix)]
fn assert_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("stat the store file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the store file must be owner-only, found {mode:o}"
    );
}

/// Permissions are process-scoped on non-Unix hosts, so there is no mode to read.
#[cfg(not(unix))]
fn assert_owner_only(_path: &Path) {}

/// Leave the store file at the mode a permissive umask produces.
#[cfg(unix)]
fn make_world_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
        .expect("make the store file world-readable");
}

/// There is no file mode to set on a non-Unix host.
#[cfg(not(unix))]
fn make_world_readable(_path: &Path) {}

#[test]
fn open_creates_the_database_owner_only() {
    let scratch = Scratch::new("create");
    let path = scratch.join("clio.db");
    let store = SqliteStore::open(&path, kms()).expect("open a file-backed store");
    drop(store);
    assert_owner_only(&path);
}

#[test]
fn open_tightens_a_world_readable_database() {
    let scratch = Scratch::new("tighten");
    let path = scratch.join("clio.db");
    // An empty file is a valid empty database, so this is the shape a store
    // created by a permissive umask has before anything is written to it.
    std::fs::write(&path, b"").expect("write the store file");
    make_world_readable(&path);
    let store = SqliteStore::open(&path, kms()).expect("open the existing store file");
    drop(store);
    assert_owner_only(&path);
}
