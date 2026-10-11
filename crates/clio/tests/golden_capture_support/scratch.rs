// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the scratch directory both golden test binaries use for
//! the files they write. No tests live here.
//!
//! ## Owns
//! - One unique temporary directory per test, removed when its guard drops
//!
//! ## Does not own
//! - Anything a test writes inside the directory (each test owns its own file)
//!
//! ## Boundary
//!
//! Changes to how a scratch directory is named or removed should normally
//! require changes here.
//!
//! Keep this module focused on one throwaway directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A unique scratch directory, removed when the guard drops. `tempfile` is not
/// a workspace dependency, so this is the repository's own fixture pattern
/// (see `sync_wire_support::NodeDir`): pid, a monotonic counter, and nanos
/// keep parallel tests apart, and the drop removes the directory on panic too.
pub struct Scratch(pub PathBuf);

impl Scratch {
    /// Create the directory; its name carries the caller's `tag` so a leftover
    /// directory after a hard kill can be traced to the test that made it.
    pub fn new(tag: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-golden-{tag}-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
