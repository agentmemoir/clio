// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the store lock's in-process behavior: hold
//! coexistence, the release on drop, the refusals this process can produce by
//! itself, the lock file's shape, and the pid record. The cross-process
//! guarantee is exercised in `process_tests`, which a process-local test
//! cannot demonstrate.
//!
//! ## Owns
//! - The store lock's unit coverage
//!
//! ## Does not own
//! - The two-process scenarios (`store_lock_process_tests`)
//! - The swap-level lock tests (`restore_swap_lock_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the store lock's own behavior.

use super::*;
use crate::store_lock_pids::recorded_pid;
use std::fs::OpenOptions;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-lock-{tag}"
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

/// A store file that exists, so a lock beside it is the ordinary case.
fn a_store(dir: &Scratch) -> PathBuf {
    let store = dir.join("clio.db");
    std::fs::write(&store, b"store").expect("write a stand-in store file");
    store
}

/// A descriptor holding the lock outside the registry, as a foreign process's
/// hold would: `flock` locks are per descriptor, so a second descriptor of
/// this process contends with the first exactly as another process would.
fn raw_holder(store: &Path, exclusive: bool) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_file_path(store))
        .expect("open the lock file for a raw holder");
    if exclusive {
        file.lock().expect("hold the lock exclusively");
    } else {
        file.lock_shared().expect("hold the lock shared");
    }
    file
}

#[test]
fn two_shared_holds_coexist() {
    let scratch = Scratch::new("two-shared");
    let store = a_store(&scratch);
    let first = SharedHold::acquire(&store).expect("first shared hold");
    let second = SharedHold::acquire(&store).expect("second shared hold");
    drop(second);
    drop(first);
}

#[test]
fn a_shared_hold_is_released_on_drop() {
    let scratch = Scratch::new("shared-drop");
    let store = a_store(&scratch);
    drop(SharedHold::acquire(&store).expect("take the shared hold"));
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the exclusive hold must be free once the shared hold is dropped");
}

#[test]
fn an_exclusive_hold_is_refused_while_this_process_holds_the_store_shared() {
    let scratch = Scratch::new("self-shared");
    let store = a_store(&scratch);
    let hold = SharedHold::acquire(&store).expect("take the shared hold");
    let error = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("this process's own shared holds must refuse the exclusive hold");
    assert!(
        error.message.contains("shared hold(s)"),
        "the refusal names the holds: {}",
        error.message
    );
    drop(hold);
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the exclusive hold is free once the shared hold is dropped");
}

#[test]
fn a_second_exclusive_hold_is_refused() {
    let scratch = Scratch::new("double-exclusive");
    let store = a_store(&scratch);
    let hold = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("take the exclusive hold");
    let error = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("a second exclusive hold must be refused");
    assert!(
        error.message.contains("already held exclusively"),
        "the refusal names the live hold: {}",
        error.message
    );
    drop(hold);
}

#[test]
fn a_shared_hold_inside_this_processs_exclusive_hold_is_nested() {
    let scratch = Scratch::new("nested-shared");
    let store = a_store(&scratch);
    let exclusive = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("take the exclusive hold");
    // The restore's own opens run inside its exclusive hold; they must go
    // through without a second flock, which would contend with it.
    let nested = SharedHold::acquire(&store).expect("a nested shared hold");
    drop(nested);
    let error = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("the exclusive hold must outlive the nested shared hold");
    assert!(
        error.message.contains("already held exclusively"),
        "{}",
        error.message
    );
    drop(exclusive);
    SharedHold::acquire(&store).expect("the store is holdable again");
}

#[test]
fn a_shared_hold_is_released_on_panic_unwind() {
    let scratch = Scratch::new("shared-panic");
    let store = a_store(&scratch);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _hold = SharedHold::acquire(&store).expect("take the shared hold");
        panic!("unwind out of the shared hold");
    }));
    assert!(
        outcome.is_err(),
        "the panic must propagate, not be swallowed"
    );
    assert!(
        !this_process_holds_shared(&store),
        "a panic must release the shared hold"
    );
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the exclusive hold must be free after the unwind");
}

#[test]
fn an_exclusive_hold_is_released_on_panic_unwind() {
    let scratch = Scratch::new("exclusive-panic");
    let store = a_store(&scratch);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _hold = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
            .expect("take the exclusive hold");
        panic!("unwind out of the exclusive hold");
    }));
    assert!(
        outcome.is_err(),
        "the panic must propagate, not be swallowed"
    );
    assert!(
        !this_process_holds_shared(&store),
        "no shared hold may survive the unwind"
    );
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the exclusive hold must be free after the unwind");
}

#[test]
fn the_lock_file_is_created_owner_only() {
    let scratch = Scratch::new("owner-only");
    let store = a_store(&scratch);
    drop(SharedHold::acquire(&store).expect("take the shared hold"));
    let mode = std::fs::metadata(lock_file_path(&store))
        .expect("the lock file exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the lock file is owner-only");
}

#[test]
fn an_open_that_cannot_create_its_lock_file_fails_with_the_stores_error_shape() {
    let scratch = Scratch::new("no-parent");
    // A store path under a file: the lock's parent cannot exist as a
    // directory, so the lock file cannot be opened.
    let store = scratch.join("clio.db").join("nested.db");
    let error = SharedHold::acquire(&store).expect_err("the lock must fail");
    assert!(
        error.message.contains("store lock"),
        "the refusal names the lock, not a bare OS error: {}",
        error.message
    );
    let error = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("the exclusive hold needs the lock file too");
    assert!(error.message.contains("store lock"), "{}", error.message);
}

#[test]
fn the_exclusive_hold_records_its_pid_and_clears_it_on_release() {
    let scratch = Scratch::new("pid-record");
    let store = a_store(&scratch);
    let hold = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("take the exclusive hold");
    let lock_path = lock_file_path(&store);
    assert_eq!(
        recorded_pid(&lock_path),
        Some(std::process::id()),
        "the lock file records the holding pid"
    );
    drop(hold);
    let left = std::fs::read_to_string(&lock_path).expect("read the lock file");
    assert!(left.is_empty(), "the pid line goes with the hold: {left:?}");
}

#[test]
fn a_shared_hold_waits_a_bounded_time_and_refuses_while_the_lock_is_held_exclusively() {
    let scratch = Scratch::new("shared-waits");
    let store = a_store(&scratch);
    let holder = raw_holder(&store, true);
    let error = SharedHold::acquire_with_wait(&store, Duration::from_millis(120))
        .expect_err("a contending shared hold must refuse");
    assert!(
        error.message.contains("a restore is replacing it"),
        "the refusal says what holds the store: {}",
        error.message
    );
    #[cfg(target_os = "linux")]
    assert!(
        error
            .message
            .contains(&format!("pid(s) {}", std::process::id())),
        "the holder's pid is named on Linux: {}",
        error.message
    );
    drop(holder);
    SharedHold::acquire(&store).expect("the hold is free once the holder is gone");
}

#[test]
fn an_exclusive_hold_waits_a_bounded_time_and_refuses_naming_the_shared_holders() {
    let scratch = Scratch::new("exclusive-waits");
    let store = a_store(&scratch);
    let holder = raw_holder(&store, false);
    let error = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(120))
        .expect_err("a contending exclusive hold must refuse");
    assert!(
        error.message.contains("held shared"),
        "the refusal says the lock is shared: {}",
        error.message
    );
    #[cfg(target_os = "linux")]
    assert!(
        error
            .message
            .contains(&format!("pid(s) {}", std::process::id())),
        "the holder's pid is named on Linux: {}",
        error.message
    );
    drop(holder);
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the hold is free once the holder is gone");
}

#[test]
fn a_stale_lock_file_does_not_block_a_hold() {
    let scratch = Scratch::new("stale-file");
    let store = a_store(&scratch);
    std::fs::write(lock_file_path(&store), b"pid 999999\n").expect("write a stale lock file");
    SharedHold::acquire(&store).expect("a stale file must not block");
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("a stale file must not block the exclusive hold either");
}

#[test]
fn the_lock_file_is_derived_from_the_stores_resolved_path() {
    let scratch = Scratch::new("resolved-path");
    let real = scratch.join("real");
    std::fs::create_dir(&real).expect("create the real directory");
    let linked = scratch.join("linked");
    std::os::unix::fs::symlink(&real, &linked).expect("symlink the directory");
    let store = real.join("clio.db");
    std::fs::write(&store, b"store").expect("write the store file");
    assert_eq!(
        lock_file_path(&linked.join("clio.db")),
        lock_file_path(&store),
        "the store and its symlinked spelling name one lock file"
    );
}
