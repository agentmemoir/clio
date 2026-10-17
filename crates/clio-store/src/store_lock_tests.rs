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
/// this process contends with the first exactly as another process would. An
/// exclusive raw holder records its pid, as the real `ExclusiveHold` does, so
/// the shared direction's refusal can name it through that record.
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
        // A real exclusive hold records its pid, and the shared direction's
        // refusal falls back to that record when `/proc/locks` reports no
        // holder. Record it here too, so this test states the promise instead
        // of depending on the platform's flock listing.
        record_pid(&file);
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
fn a_lock_beside_a_missing_directory_creates_its_parent() {
    let scratch = Scratch::new("missing-parent");
    // Neither the store nor its directory exists yet: the first hold under a
    // fresh directory (a first erasure under a fresh data directory) creates
    // the parent instead of failing closed on it.
    let store = scratch.join("fresh-dir").join("clio.db");
    drop(SharedHold::acquire(&store).expect("a missing lock parent is created"));
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the created parent holds the exclusive hold too");
}

#[test]
fn an_open_that_cannot_create_its_lock_file_fails_with_the_stores_error_shape() {
    let scratch = Scratch::new("no-parent");
    // A store path under a file: the lock's parent cannot exist as a
    // directory, so the lock file cannot be opened.
    let obstruction = scratch.join("clio.db");
    std::fs::write(&obstruction, b"not a directory").expect("plant the obstructing file");
    let store = obstruction.join("nested.db");
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
    // The restoring process's pid is a promise: the exclusive hold records
    // it, and the shared direction falls back to that record when the
    // platform reports nothing, so this holds on every platform.
    assert!(
        error.message.contains(&format!(
            "held exclusively by pid(s) {};",
            std::process::id()
        )),
        "the restoring process is named from the pid it recorded: {}",
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
    // This process's own raw descriptor is the holder, and it cannot exit
    // between the lock attempt and the lookup, so the in-process flock
    // listing names it; a cross-process holder is best effort (see
    // `store_lock`).
    #[cfg(target_os = "linux")]
    assert!(
        error
            .message
            .contains(&format!("held shared by pid(s) {};", std::process::id())),
        "the in-process shared holder is named: {}",
        error.message
    );
    drop(holder);
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the hold is free once the holder is gone");
}

/// The refusal is complete when the platform reports no holder: naming a
/// shared holder is best effort, so an empty lookup still names the lock and
/// the contention and adds no pid. This pins the outcome the Linux e2e test
/// once treated as a failure (issue #156).
#[test]
fn a_refusal_with_no_reported_holder_names_the_lock_without_pids() {
    let scratch = Scratch::new("no-holder");
    let store = a_store(&scratch);
    // The lock file exists but nothing holds it, so the holder lookup finds
    // no entry for it on any platform.
    drop(SharedHold::acquire(&store).expect("create the lock file"));
    let error = Mode::Exclusive.refused(&store, &lock_file_path(&store));
    assert!(
        error.message.contains("held shared"),
        "the refusal names the contention: {}",
        error.message
    );
    assert!(
        error.message.contains("store lock"),
        "the refusal names the lock: {}",
        error.message
    );
    assert!(
        !error.message.contains("by pid(s)"),
        "no holder is reported, so no pid is named: {}",
        error.message
    );
}

/// The refusal text is a promise given a holder list, split from the platform
/// lookup so both branches are deterministic: an empty list adds no pid, and
/// a non-empty list names every pid in order. This pins the named branch that
/// the cross-process tests may not reach (issue #156).
#[test]
fn a_refusal_names_every_reported_holder_and_omits_an_empty_list() {
    let scratch = Scratch::new("refusal-text");
    let store = a_store(&scratch);
    let lock = lock_file_path(&store);
    let named = Mode::Exclusive.refusal(&store, &lock, &[4242, 4243]);
    assert!(
        named.message.contains("held shared by pid(s) 4242, 4243;"),
        "the refusal names every reported holder in order: {}",
        named.message
    );
    let none = Mode::Exclusive.refusal(&store, &lock, &[]);
    assert!(
        !none.message.contains("by pid(s)"),
        "an empty holder list names no pid: {}",
        none.message
    );
    let shared = Mode::Shared.refusal(&store, &lock, &[7]);
    assert!(
        shared.message.contains("held exclusively by pid(s) 7;"),
        "the shared direction names its holder too: {}",
        shared.message
    );
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
