// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite restore's projected pre-restore snapshot
//! footprint: the target file's bytes plus the key store the snapshot copies,
//! and nothing at all when there is no target to snapshot.
//!
//! ## Owns
//! - `projected_snapshot_bytes`, the room estimate the pre-restore snapshot
//!   makes
//!
//! ## Does not own
//! - `plan_sqlite`, which calls it (see `backup_restore_sqlite`)
//! - The space check itself (see `clio-store`'s restore pre-flight)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::projected_snapshot_bytes;
use crate::backup_cli_fixtures::Scratch;
use clio_store::SqliteFileState;
use std::path::PathBuf;

/// A SQLite file state reporting `bytes` for a file that may or may not exist.
fn state(exists: bool, bytes: u64) -> SqliteFileState {
    SqliteFileState {
        path: PathBuf::from("/nonexistent/clio.db"),
        exists,
        bytes,
        table_counts: Vec::new(),
        banks: Vec::new(),
    }
}

/// A key-store file of `bytes` bytes inside `dir`.
fn key_store(dir: &Scratch, bytes: usize) -> PathBuf {
    let path = dir.join("clio.keys");
    std::fs::write(&path, vec![0u8; bytes]).expect("write a key-store file");
    path
}

#[test]
fn the_projection_adds_the_key_store_to_the_target_bytes() {
    let dir = Scratch::new("restore-footprint-sqlite");
    let keys = key_store(&dir, 4096);
    assert_eq!(
        projected_snapshot_bytes(&state(true, 10_000), Some(&keys)),
        14_096,
        "the key store is part of what the pre-restore snapshot copies"
    );
}

#[test]
fn the_projection_is_the_target_bytes_when_there_is_no_key_store() {
    let dir = Scratch::new("restore-footprint-nokeys");
    let missing = dir.join("no-such.keys");
    assert_eq!(projected_snapshot_bytes(&state(true, 10_000), None), 10_000);
    assert_eq!(
        projected_snapshot_bytes(&state(true, 10_000), Some(&missing)),
        10_000
    );
}

#[test]
fn the_projection_reserves_nothing_when_there_is_no_target() {
    let dir = Scratch::new("restore-footprint-absent");
    let keys = key_store(&dir, 4096);
    // No target file means no snapshot is taken, so nothing is reserved.
    assert_eq!(projected_snapshot_bytes(&state(false, 0), Some(&keys)), 0);
    assert_eq!(projected_snapshot_bytes(&state(false, 0), None), 0);
}
