// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one shared scratch-SQLite fixture for the
//! per-peer request suites: the scratch directory that removes itself, the
//! local key provider, and the file-backed store opened at a caller's path, so
//! a restart is a reopen of the same file.
//!
//! ## Owns
//! - `Scratch`, the per-test scratch directory and its drop
//! - `kms`, the local development key provider
//! - `open_sqlite`, the file-backed scratch store opener
//!
//! ## Does not own
//! - The peer lifecycle (`subject_peer.rs`) and the peer-denial producer
//!   (`subject_request_peer_events.rs`); the suites over them own their
//!   assertions
//! - The Postgres fixture: the scoped-schema helper lives in
//!   `inventory_pg_support_tests`
//!
//! ## Boundary
//!
//! Changes to how a scratch store is opened should normally require changes
//! here. Changes to what a peer suite asserts should not.
//!
//! Test-only. Never compiled into production builds.

use crate::test_unique::unique;
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A key provider for the scratch stores.
pub(crate) fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

/// A scratch directory removed when the guard drops.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        Self(std::env::temp_dir().join(unique("clio-compliance-peer-scratch")))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Open the file-backed SQLite store at `path`; a reopen is the restart.
pub(crate) fn open_sqlite(path: &Path) -> SqliteStore {
    SqliteStore::open(path, kms()).expect("scratch sqlite opens")
}
