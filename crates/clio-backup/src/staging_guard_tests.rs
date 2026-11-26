// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing who removes a staged path, and when:
//! the reserved name that is only handed out, the guard that removes a staged
//! file nothing adopted, and the single handoff that makes one value its owner.
//!
//! ## Owns
//! - The reservation of a name for a staged member
//! - The removal `StagedGuard` performs when it is dropped unreleased, and the
//!   removal it stops performing once it has released the path
//!
//! ## Does not own
//! - Staging a member and digesting it (see `staging_tests`)
//! - The backup directory's creation and its mode (see `staging_dir_tests`)
//! - The atomic publish (see `atomic_publish_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the ownership of a staged path, not on the bytes a
//! staged member holds.

use super::*;
use crate::fixtures::Scratch;

#[test]
fn a_reserved_path_does_not_exist_yet() {
    let scratch = Scratch::new("staging-reserve");

    let path = reserve_path(scratch.path(), "payload").expect("reserve");

    assert!(
        !path.exists(),
        "VACUUM INTO needs a path that does not exist"
    );
    assert!(path.starts_with(scratch.path()));
    // A second reservation in the same instant gets a different name.
    let again = reserve_path(scratch.path(), "payload").expect("reserve again");
    assert_ne!(path, again);
}

/// A guard that was never released removes the staged file, which is the whole
/// point of wrapping the path: a copy that fails cannot leave the file behind.
#[test]
fn an_unreleased_staged_path_is_removed_when_the_guard_drops() {
    let scratch = Scratch::new("staging-guard-drop");
    let path = reserve_path(scratch.path(), "member").expect("reserve");
    std::fs::write(&path, b"key material").expect("write the staged bytes");
    assert!(path.exists());

    drop(StagedGuard::new(path.clone()));

    assert!(
        !path.exists(),
        "a guard that never released removes its file"
    );
}

/// A released path survives the guard, so the returned `StagedFile` — and only
/// it — removes the file, once, when the run ends.
#[test]
fn a_released_staged_path_survives_the_guard_and_is_removed_by_its_staged_file() {
    let scratch = Scratch::new("staging-guard-release");
    let path = reserve_path(scratch.path(), "member").expect("reserve");
    std::fs::write(&path, b"key material").expect("write the staged bytes");

    let released = StagedGuard::new(path.clone()).release();

    assert_eq!(released, path, "release hands the same path on");
    assert!(
        path.exists(),
        "a released path is not removed by the guard that held it"
    );

    let staged = StagedFile::adopt(released, 12, "a".repeat(64));
    drop(staged);
    assert!(!path.exists(), "the staged file removes it exactly once");
}
