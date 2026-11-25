// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing which entry a key removal may unlink: the
//! file the caller read, never one that appeared at the path afterwards, and never a
//! link or a directory planted there.
//!
//! ## Owns
//! - Every test of `remove_key_holding`
//! - The refusal an entry that changed after it was archived reports
//!
//! ## Does not own
//! - The read side and the missing-file message (see
//!   `sync_key_file_identity_tests`)
//! - The archive and the rotation that calls this (see `sync_key_rotation_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a
//! unique temp directory removed when the guard drops, and no test waits, sleeps,
//! or starts a thread.
//!
//! Keep this module focused on the removal's refusal rules.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::sync_key_file::{read_key_holding, sidecar_path, write_created, write_key};
use crate::sync_key_test_support::TempDir;
use std::fs;

// ------------------------------------------------------- the archived entry

#[test]
fn the_held_key_is_removed_with_its_sidecar() {
    let dir = TempDir::new("identity-held");
    write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-11-01T00:00:00Z").expect("record");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");

    remove_key_holding(&dir.key(), &held).expect("remove the file that was read");

    assert!(!dir.key().exists(), "the archived key file is gone");
    assert!(
        !sidecar_path(Some(&dir.key()))
            .expect("sidecar path")
            .exists(),
        "the creation record describes the key that was just let go of"
    );
}

#[test]
fn an_entry_already_gone_is_nothing_to_remove() {
    let dir = TempDir::new("identity-gone");
    write_key(&dir.key(), &[11_u8; KEY_LEN]).expect("write");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    fs::remove_file(dir.key()).expect("drop the entry");

    remove_key_holding(&dir.key(), &held).expect("an absent entry is not a failure");

    assert!(!dir.key().exists());
}

// ---------------------------------------------------- an entry that changed

#[cfg(unix)]
#[test]
fn a_replacement_key_is_refused_and_left_unchanged() {
    let dir = TempDir::new("identity-replaced");
    write_key(&dir.key(), &[8_u8; KEY_LEN]).expect("write");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    let replacement = [9_u8; KEY_LEN];
    fs::remove_file(dir.key()).expect("let go of the entry");
    write_key(&dir.key(), &replacement).expect("write the replacement");

    let err =
        remove_key_holding(&dir.key(), &held).expect_err("a new entry is not the archived key");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(
        fs::read(dir.key()).expect("read back"),
        replacement,
        "the replacement is still there, byte for byte"
    );
}

#[cfg(unix)]
#[test]
fn a_planted_link_is_refused_and_neither_entry_is_removed() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new("identity-link");
    write_key(&dir.key(), &[10_u8; KEY_LEN]).expect("write");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    let moved = dir.join("moved.key");
    fs::rename(dir.key(), &moved).expect("move the archived key aside");
    symlink(&moved, dir.key()).expect("plant the link");

    let err = remove_key_holding(&dir.key(), &held).expect_err("a link is not the archived entry");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(
        dir.key()
            .symlink_metadata()
            .expect("link")
            .file_type()
            .is_symlink(),
        "the planted link is untouched"
    );
    assert!(
        moved.exists(),
        "the file the link points at is untouched too"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_at_the_path_is_refused() {
    let dir = TempDir::new("identity-dir");
    write_key(&dir.key(), &[12_u8; KEY_LEN]).expect("write");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    fs::remove_file(dir.key()).expect("let go of the entry");
    fs::create_dir(dir.key()).expect("plant a directory");

    let err =
        remove_key_holding(&dir.key(), &held).expect_err("a directory is not the archived entry");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(dir.key().is_dir(), "the directory is untouched");
}
