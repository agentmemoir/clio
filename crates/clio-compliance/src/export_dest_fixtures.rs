// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns the bundle destination paths the export test suites write
//! to: one unique path under the system temp dir per call, removed when the
//! guard that holds it drops.
//!
//! ## Owns
//! - `DestPath`, the Drop-guarded destination path
//! - `dest_path`, the collision-free name builder over the shared counter
//!
//! ## Does not own
//! - The export fixtures and drivers that consume a destination (see
//!   `export_tests.rs`)
//! - The export writer itself (see `export.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_unique::unique;
use std::path::{Path, PathBuf};

/// A bundle destination under the system temp dir, removed when it drops.
///
/// Every call returns a path no other test shares, so parallel tests never read
/// each other's bundle. The guard owns that path, so a finished test leaves
/// nothing behind under the shared temp root: `Fixture` holds one, and a test
/// that builds its own export arguments can hold one directly.
///
/// The string form is built once, at construction, so `ExportArgs` can borrow it
/// for as long as it borrows the fixture, and a path whose bytes are not UTF-8 is
/// converted lossily rather than panicking.
pub(crate) struct DestPath {
    path: PathBuf,
    text: String,
}

impl DestPath {
    /// The destination path, for reading the written bundle back.
    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }

    /// The destination as a string, for `ExportArgs::destination`.
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }
}

impl Drop for DestPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A unique `am-export-<name>-*.json` path under the system temp dir.
pub(crate) fn dest_path(name: &str) -> DestPath {
    let path = std::env::temp_dir().join(format!("am-export-{}.json", unique(name)));
    let text = path.to_string_lossy().into_owned();
    DestPath { path, text }
}
