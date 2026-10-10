// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared scaffolding the sync key's test
//! modules need: a self-cleaning directory per test and an injected environment
//! lookup built from key/value pairs.
//!
//! ## Owns
//! - [`TempDir`], a unique directory removed when the guard drops
//! - [`env`], a closure that answers for named variables and nothing else
//!
//! ## Does not own
//! - Any test (see `sync_key_tests`, `sync_key_marker_tests`,
//!   `sync_key_env_tests`, and `sync_key_file_tests`)
//! - The injected keystore credential (see `secret_key_store_test_support`)
//!
//! ## Boundary
//!
//! There is no constructor here that reads the process environment or the
//! machine's data directory. Every caller passes its own values, so a test can
//! neither write to the machine it runs on nor read another test's directory.
//!
//! Keep this module focused on test scaffolding, with no tests of its own.

use std::path::{Path, PathBuf};

/// A directory unique per call, removed when the guard drops.
///
/// The name carries the test's label, the process id, and a nanosecond stamp, so
/// two tests in the same process, two processes on one machine, and two machines
/// cannot collide. `tempfile` is not a dependency of this crate, and a fixed
/// shared path would be exactly the coupling the rule forbids.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// A fresh directory for `label`.
    pub(crate) fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "clio-sync-key-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the temp directory can be created");
        Self(path)
    }

    /// The directory itself.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the directory, by name.
    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The sync key file inside it, under its production name.
    pub(crate) fn key(&self) -> PathBuf {
        self.0.join(crate::sync_key_file::SYNC_KEY_FILE_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An injected environment lookup from key/value pairs.
///
/// A key that is not listed answers `None`, so a test never sees the process's
/// own environment. A value is returned verbatim, including a blank one, so
/// each caller decides for itself: the production getters trim and drop a
/// blank.
pub(crate) fn env(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    }
}

/// The shared environment built from borrowed pairs, for a test whose values
/// are literals or that keeps its own values alive.
pub(crate) fn env_of(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(&'static str, String)> = pairs
        .iter()
        .map(|(name, value)| (*name, (*value).to_owned()))
        .collect();
    move |key: &str| {
        owned
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    }
}
