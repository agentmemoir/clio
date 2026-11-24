// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared deployment salt test fixtures: a
//! scratch directory, the sidecar path, and a seeded owner-only file.
//!
//! ## Owns
//! - `Scratch`, `sidecar`, `seed_owner_only`, `salt_of`, and `set_mode`
//!
//! ## Does not own
//! - The assertions about the salt (`salt_tests.rs` owns those)
//! - The refusal for a swapped sidecar (`salt_swap_tests.rs` owns that)
//!
//! ## Boundary
//!
//! Changes to the state a salt test starts from should normally require changes
//! here. Changes to what a salt test asserts should not.
//!
//! Test-only. Never compiled into production builds.

use std::path::{Path, PathBuf};

use crate::test_unique::unique;

/// A unique scratch directory removed on drop.
pub(crate) struct Scratch {
    /// The directory every file a test seeds lives in.
    pub(crate) root: PathBuf,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-compliance-salt"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    /// The journal path a sidecar sits beside.
    pub(crate) fn journal(&self) -> PathBuf {
        self.root.join("erasure-journal.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The owner-only sibling a salt is persisted at for `journal`.
pub(crate) fn sidecar(journal: &Path) -> PathBuf {
    let mut name = journal.as_os_str().to_os_string();
    name.push(".salt");
    PathBuf::from(name)
}

/// Seed a raw file restricted to the owner, so a length assertion is not
/// pre-empted by whatever the platform's default mode happens to be.
pub(crate) fn seed_owner_only(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("seed the raw file");
    #[cfg(unix)]
    set_mode(path, 0o600);
}

/// A byte string of `len` copies of `byte`, derived rather than written out, so
/// a seeded deployment salt is computed on every run and reproducible.
pub(crate) fn salt_of(byte: u8, len: usize) -> Vec<u8> {
    (0..len).map(|_| byte).collect()
}

/// Restrict `path` to `mode`, so a length or refusal assertion is not pre-empted
/// by whatever the platform's default mode happens to be.
#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|err| panic!("set mode {mode:o} on {}: {err}", path.display()));
}
