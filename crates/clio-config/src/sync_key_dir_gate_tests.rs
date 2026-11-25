// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the one rule the directory holding key
//! material has to pass: a link or a non-directory is refused, a real directory is
//! tightened to `0700` through the descriptor that was judged, and a directory
//! replaced after that open is refused instead of reported as tightened.
//!
//! ## Owns
//! - The swap case: two directories, one opened and then renamed away, and the
//!   entry at the name replaced before the mode is set
//! - The link case at the directory's final name, and the mode its target keeps
//! - The happy path, the parent that is a regular file, the parent that is gone,
//!   and the process directory that is never tightened
//!
//! ## Does not own
//! - A FIFO or a link at the key file's own final name (see `sync_key_gate_tests`)
//! - Where the key file lives or what the sidecar holds (see
//!   `sync_key_file_tests` and `sync_key_rotation_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a
//! unique temp directory removed when the guard drops, so nothing here reaches the
//! machine's real key material and no two tests can collide.
//!
//! Keep this module focused on the directory rule.

// Every case here plants a link, a foreign inode, or a mode, none of which a
// non-Unix build can express, so the whole module is Unix-only.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use crate::sync_key_test_support::TempDir;

use super::{ensure_owner_only_dir, open_key_dir, tighten_dir};

/// The mode bits of a path, read through the path.
fn mode_at(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("the path can be read")
        .permissions()
        .mode()
        & 0o777
}

/// Seed a directory at `path` with a mode a reader can see.
fn seed_dir(path: &std::path::Path, mode: u32) {
    std::fs::create_dir(path).expect("the directory can be created");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

// ------------------------------------------------------------------- the swap

#[test]
fn a_directory_swapped_after_its_open_keeps_the_mode_and_is_refused() {
    // The invariant this pins: the `0700` lands on the directory Clio opened, and
    // never on whatever is standing at the name when the mode is set, and the
    // entry that is standing there is refused rather than reported as tightened.
    //
    // The swap is done in process, between the open and the mode change, through
    // the two steps the gate is built from. Against the three-lookup version this
    // case cannot even be expressed: there the mode is set by path, so the same
    // rename would put `0700` on the replacement — the directory Clio never
    // judged — and the read-back would then approve it.
    let holder = TempDir::new("dir-gate-swap");
    let keys = holder.join("keys");
    let moved = holder.join("keys-opened");
    seed_dir(&keys, 0o755);

    let opened = open_key_dir(&keys).expect("the real directory opens");
    std::fs::rename(&keys, &moved).expect("the opened directory is moved away");
    seed_dir(&keys, 0o755);

    let err = tighten_dir(&opened, &keys)
        .expect_err("the entry at the name is not the directory that was opened");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let expected = format!(
        "sync key directory {}: refused because it was replaced while Clio was checking it; \
         replace it with a real directory only the owner can enter and run again",
        keys.display()
    );
    assert_eq!(err.message, expected, "the refusal names the replacement");
    assert_eq!(
        mode_at(&moved),
        0o700,
        "the directory Clio opened is the one the chmod landed on"
    );
    assert_eq!(
        mode_at(&keys),
        0o755,
        "the swapped-in directory was left exactly as it was"
    );
}

#[test]
fn the_same_directory_is_not_refused_for_having_been_opened() {
    // The other half of the swap case: the dev/ino comparison is a replacement
    // check and not a rule that fails every directory, so a directory nobody moved
    // is tightened and accepted.
    let holder = TempDir::new("dir-gate-steady");
    let keys = holder.join("keys");
    seed_dir(&keys, 0o755);

    let opened = open_key_dir(&keys).expect("the real directory opens");
    tighten_dir(&opened, &keys).expect("the directory that was opened is the one at the name");

    assert_eq!(
        mode_at(&keys),
        0o700,
        "it was tightened through the descriptor"
    );
}

// ------------------------------------------------------------------- the link

#[test]
fn a_symlinked_key_directory_is_refused_and_its_target_keeps_its_mode() {
    // A link at the directory's final name is refused by the open itself, so the
    // real directory behind it is never opened and never tightened: the failure is
    // a refusal, not a `chmod` that followed the link.
    let outside = TempDir::new("dir-gate-link-target");
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    let holder = TempDir::new("dir-gate-link");
    let linked = holder.join("keys");
    std::os::unix::fs::symlink(outside.path(), &linked).expect("plant the directory link");

    let err = ensure_owner_only_dir(&linked).expect_err("a link is not a directory to tighten");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let expected = format!(
        "sync key directory {}: refused because it is a symbolic link; replace it with a real \
         directory only the owner can enter and run again",
        linked.display()
    );
    assert_eq!(err.message, expected);
    assert_eq!(
        mode_at(outside.path()),
        0o755,
        "the target of the link kept its mode: nothing was chmodded through the link"
    );
}

// ------------------------------------------------------------------ the happy

#[test]
fn a_loose_key_directory_is_tightened_to_owner_only() {
    // The happy path: a real directory a reader can see is tightened to `0700`, and
    // the mode is read back from the descriptor that set it rather than assumed.
    let holder = TempDir::new("dir-gate-happy");
    let keys = holder.join("keys");
    seed_dir(&keys, 0o755);

    ensure_owner_only_dir(&keys).expect("a real directory is tightened");

    assert_eq!(
        mode_at(&keys),
        0o700,
        "the directory is owner-only afterwards"
    );
}

// --------------------------------------------------------------- wrong shape

#[test]
fn a_key_directory_that_is_a_regular_file_is_refused() {
    // A parent that exists but is not a directory is refused by the open itself.
    // The gate is called directly here, because a call through `write_created` or
    // `append_entry` reaches `create_dir_all` first, and that fails before the
    // gate's non-directory arm is ever asked.
    let dir = TempDir::new("dir-gate-file");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("blocker");

    let err =
        ensure_owner_only_dir(&blocker).expect_err("a regular file is not a directory to tighten");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let expected = format!(
        "sync key directory {}: refused because it is not a directory; replace it with a real \
         directory only the owner can enter and run again",
        blocker.display()
    );
    assert_eq!(err.message, expected);
    assert_eq!(
        std::fs::read_to_string(&blocker).expect("the blocker is untouched"),
        "not a directory",
        "the refusal neither chmodded nor wrote through the path it refused"
    );
}

#[test]
fn a_missing_key_directory_is_refused_before_it_is_tightened() {
    // The gate opens the parent before it tightens it, so a path that is gone is a
    // reported failure rather than key material kept in a directory no one checked.
    let holder = TempDir::new("dir-gate-gone");
    let missing = holder.join("absent");

    let err =
        ensure_owner_only_dir(&missing).expect_err("a directory that cannot be opened is refused");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        err.message.starts_with(&format!(
            "cannot read the Clio sync key directory {}: ",
            missing.display()
        )),
        "the failure names the unopenable directory: {}",
        err.message
    );
}

#[test]
fn a_path_with_no_directory_of_its_own_is_never_tightened() {
    // A bare relative key name reaches the gate as `.`. The early return is
    // unchanged: a path whose components are all `.` or `..` names no directory a
    // key setting chose, so it is accepted untouched. Because the return runs
    // before any open, the gate reads and writes nothing here, which is what lets
    // this case run in any order, in any directory, next to every other case in
    // this module rather than moving the process somewhere else.
    for unchanged in [".", "..", "./.", "./"] {
        ensure_owner_only_dir(std::path::Path::new(unchanged))
            .unwrap_or_else(|err| panic!("{unchanged} is left alone: {}", err.message));
    }
}
