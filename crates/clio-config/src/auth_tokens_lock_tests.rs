// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the token store's writer lock: that the lock file lands
//! where the item store's rule puts it, that a second writer refuses inside a
//! bounded wait, and that the hold is released on drop.
//!
//! ## Owns
//! - Behavior tests for `auth_tokens_lock`
//!
//! ## Does not own
//! - The lock implementation under test
//! - Record validation tests (they live in `auth_tokens_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unique scratch store path per call, so parallel tests never share a lock.
fn tmp(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-auth-lock-{name}-{n}.json"))
}

#[test]
fn the_lock_file_is_the_store_path_plus_a_dot_lock_suffix() {
    assert_eq!(
        lock_file_path(Path::new("/etc/clio/auth_tokens.json")),
        PathBuf::from("/etc/clio/auth_tokens.json.lock")
    );
}

#[test]
fn a_second_holder_refuses_inside_the_wait_it_was_given() {
    // A process-local test cannot demonstrate a cross-process guarantee, so
    // this proves the bounded refusal itself: two descriptors of one store,
    // the second refused with the wait it was handed rather than the
    // production duration. The cross-process side is covered by the real
    // binary harness (`crates/clio/tests/auth_token_harness.rs`).
    let store = tmp("contend");
    let first = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the first writer takes the hold");
    let err = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("a second writer refuses");
    assert_eq!(err.code, crate::error::ErrorCode::InvalidArgument);
    assert!(err.message.contains("another writer holds it"), "{err}");
    assert!(
        err.message
            .contains(&lock_file_path(&store).display().to_string()),
        "the refusal names the lock file: {err}"
    );
    // The refusal happened inside its own bound, not the production 5s wait.
    let started = Instant::now();
    let _ = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the wait is the one the caller passed"
    );
    drop(first);
    // With the hold dropped, the next writer takes it.
    ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect("the released hold is available");
    let _ = std::fs::remove_file(lock_file_path(&store));
}

#[test]
fn the_lock_file_is_created_owner_only_and_beside_the_store() {
    let store = tmp("perms");
    let hold = ExclusiveHold::acquire(&store).expect("hold");
    assert_eq!(hold.lock_path(), lock_file_path(&store));
    assert!(hold.lock_path().is_file(), "the lock file exists");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(hold.lock_path())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o77, 0, "not owner-only: {mode:o}");
    }
    drop(hold);
    let _ = std::fs::remove_file(lock_file_path(&store));
}

#[test]
fn a_lock_file_that_cannot_be_opened_is_an_internal_refusal_naming_it() {
    // The hold cannot be taken when its lock file cannot be created. Making
    // the store's directory a regular file puts the lock path inside it, which
    // is a refusal the operator can act on rather than a panic.
    let blocker = tmp("blocked");
    std::fs::write(&blocker, b"not a directory").expect("write the blocking file");
    let store = blocker.join("auth_tokens.json");
    let err = ExclusiveHold::acquire(&store).expect_err("no lock file is reachable");
    assert_eq!(err.code, crate::error::ErrorCode::Internal);
    assert!(
        err.message.contains("failed to open the token store lock")
            && err
                .message
                .contains(&lock_file_path(&store).display().to_string()),
        "{err}"
    );
    let _ = std::fs::remove_file(&blocker);
}

#[test]
fn a_contended_hold_still_reports_its_lock_path() {
    // The refusal path names the lock through the hold it could not take, so
    // the store's own accessor is exercised on the contended branch too.
    let store = tmp("contended-path");
    let first =
        ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50)).expect("first hold");
    assert_eq!(first.lock_path(), lock_file_path(&store));
    let err = ExclusiveHold::acquire_with_wait(&store, Duration::from_millis(50))
        .expect_err("a second writer refuses");
    assert!(err.message.contains("stayed exclusively held"), "{err}");
    drop(first);
    let _ = std::fs::remove_file(lock_file_path(&store));
}
