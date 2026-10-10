// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what the swap does with the
//! cross-process store lock: the lock file survives a successful swap, the
//! exclusive hold is gone when the swap returns, and this process's own open
//! store still refuses the swap with the existing open-handle message.
//!
//! ## Owns
//! - These three swap-and-lock assertions
//!
//! ## Does not own
//! - Cross-process contention: a process-local test cannot demonstrate a
//!   cross-process guarantee, so contention is deliberately not faked here
//! - The store lock itself (see `store_lock` and its tests)
//!
//! ## Boundary
//!
//! Keep this file focused on the swap's use of the store lock.

use super::tests::{Scratch, populated_store, sidecar, staged_payload};
use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use std::sync::Arc;

/// A swap over a populated store leaves the store lock file in place.
///
/// The sidecar removal must never treat `.lock` as one of the database's
/// sidecars, and the swap must never delete the lock file: the next store
/// open in any process locks that same file again. The bytes are the lock's
/// own business — the exclusive hold rewrites the owner record while it is
/// held and truncates it on release — so existence is what is pinned here,
/// not content.
#[test]
fn a_successful_swap_leaves_the_store_lock_file_in_place() {
    let scratch = Scratch::new("lock-file");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 5);
    let lock = crate::store_lock::lock_file_path(&target);
    std::fs::write(&lock, b"pid 999999\n").expect("seed a stale lock file");

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect("swap");

    assert!(!report.is_partial(), "the swap completed");
    assert!(lock.exists(), "the lock file survives the swap");
    assert!(
        !report.sidecars_removed.iter().any(|path| path == &lock),
        "the lock file is not a sidecar: {:?}",
        report.sidecars_removed
    );
}

/// The exclusive hold is gone when the swap returns, so the store is free for
/// the next holder in any process: nothing keeps the exclusive hold alive
/// past the post-rename measurement.
#[test]
fn the_exclusive_hold_is_released_when_the_swap_returns() {
    let scratch = Scratch::new("hold-released");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 5);

    swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect("swap");

    // `acquire_with_wait` is private to `store_lock`, which this file does
    // not own, so the default bounded wait is used: it is granted at once
    // when nothing holds the store, and a leaked hold fails here after that
    // bounded wait instead of hanging.
    let hold = crate::store_lock::ExclusiveHold::acquire(&target)
        .expect("the store is free once the swap has returned");
    drop(hold);
}

/// The swap still refuses while this process holds the store open, and the
/// refusal is the existing open-handle one, with nothing replaced: when this
/// process holds the store, the exclusive hold cannot be taken past its own
/// shared holds, so the self-check refuses before the lock is even attempted.
#[test]
fn an_open_store_in_this_process_still_refuses_the_swap() {
    let scratch = Scratch::new("lock-self");
    let target = scratch.join("clio.db");
    populated_store(&target, 3);
    let staged = staged_payload(&scratch.0, 9);

    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let held = SqliteStore::open(&target, kms).expect("hold the target open");
    let before = std::fs::read(&target).expect("read the target");
    // Seeded after the store is open: a stray `-wal` makes SQLite open the
    // file in WAL mode and rewrite its header, which has nothing to do with us.
    let wal = sidecar(&target, "-wal");
    std::fs::write(&wal, b"stale sidecar bytes").expect("seed a sidecar");

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("an open store must refuse the swap");
    assert!(
        error.message.contains("open store handle"),
        "the refusal is the open-handle one: {}",
        error.message
    );

    drop(held);
    assert_eq!(std::fs::read(&target).expect("read the target"), before);
    assert!(wal.exists(), "no sidecar was touched");
    assert!(staged.exists(), "the staged payload was not consumed");
}
