// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared scaffolding the sync key CLI tests
//! need: a self-cleaning directory per test, an injected environment built from
//! key/value pairs, and the two helpers both key-verb suites use to reach a
//! stored key.
//!
//! ## Owns
//! - `TempDir`, a unique directory removed when the guard drops
//! - `env_of`, a closure that answers for named variables and nothing else
//! - `stored_key`, which puts a real key in the file store and records it
//! - `tokens`, the argument-vector helper the parser takes
//!
//! ## Does not own
//! - Any test (see `sync_key_cli_tests` and `sync_key_cli_rotation_tests`)
//! - The key lifecycle itself (see `clio-config`'s `sync_key`)
//!
//! ## Boundary
//!
//! There is no constructor here that reads the process environment. Every caller
//! passes its own values, so a test can neither write to the machine it runs on
//! nor read another test's directory.
//!
//! Keep this module focused on test scaffolding, with no tests of its own.

use clio_config::{KeySource, SYNC_KEY_FILE_NAME};
use std::fs;
use std::path::PathBuf;

/// A directory unique per call, removed when the guard drops.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "clio-sync-key-cli-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("the temp directory can be created");
        Self(path)
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }

    pub(crate) fn key(&self) -> PathBuf {
        self.0.join(SYNC_KEY_FILE_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An injected environment from key/value pairs: blank counts as a value here.
pub(crate) fn env_of(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    }
}

/// Store a file key under `env`, recording the store, and hand it back.
pub(crate) fn stored_key(env: &dyn Fn(&str) -> Option<String>) -> clio_config::LoadedSyncKey {
    let get = |key: &str| env(key);
    let loaded = clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File))
        .expect("generate");
    clio_config::sync_key::record_source(&get, KeySource::File).expect("record");
    loaded
}

/// The argument vector the parser takes, so a test writes `&["key", "export"]`.
pub(crate) fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}
