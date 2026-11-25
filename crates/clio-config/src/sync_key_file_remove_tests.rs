// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing which entry a key removal may unlink: the file the caller read, never one that appeared at the path afterwards, never a link or a directory planted there, and never a creation record that describes another key.
//!
//! ## Owns
//! - Every test of `remove_entry` and `remove_key_holding`
//! - The refusal an entry whose identity the removal cannot confirm reports
//! - Which creation record a removal is allowed to let go of
//!
//! ## Does not own
//! - The read side and the missing-file message (see `sync_key_file_identity_tests`)
//! - The archive and the rotation that calls this (see `sync_key_rotation_gen_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a unique temp directory removed when the guard drops, and no test waits, sleeps, or starts a thread.
//!
//! Keep this module focused on the removal's refusal rules.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::sync_key_file::{
    CREATED_NAME, read_created, read_key_holding, sidecar_path, write_created, write_key,
};
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

    remove_key_holding(&dir.key(), &held, Some("2026-11-01T00:00:00Z"))
        .expect("remove the file that was read");

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

    remove_key_holding(&dir.key(), &held, None).expect("an absent entry is not a failure");

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

    let err = remove_key_holding(&dir.key(), &held, None)
        .expect_err("a new entry is not the archived key");

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

    let err =
        remove_key_holding(&dir.key(), &held, None).expect_err("a link is not the archived entry");

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

    let err = remove_key_holding(&dir.key(), &held, None)
        .expect_err("a directory is not the archived entry");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(dir.key().is_dir(), "the directory is untouched");
}

#[cfg(not(unix))]
#[test]
fn an_identity_this_platform_cannot_confirm_is_refused_and_left_in_place() {
    let dir = TempDir::new("identity-unconfirmed");
    let key = [14_u8; KEY_LEN];
    write_key(&dir.key(), &key).expect("write");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");

    let err = remove_key_holding(&dir.key(), &held, None)
        .expect_err("an unconfirmed identity is not an identity to unlink");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(
        err.message.contains("left the entry in place"),
        "the refusal says the entry is still there: {}",
        err.message
    );
    assert_eq!(
        fs::read(dir.key()).expect("read back"),
        key,
        "the entry is still there, byte for byte"
    );
}

// --------------------------------------------------- the creation sidecar

#[test]
fn a_sidecar_that_names_another_key_is_left_behind() {
    let dir = TempDir::new("identity-sidecar-other");
    write_key(&dir.key(), &[15_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-11-01T00:00:00Z").expect("record the archived key");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    write_created(Some(&dir.key()), "2026-11-02T00:00:00Z").expect("record the replacement");

    remove_key_holding(&dir.key(), &held, Some("2026-11-01T00:00:00Z"))
        .expect("the key file is still the one that was read");

    assert!(!dir.key().exists(), "the archived key file is gone");
    assert_eq!(
        read_created(Some(&dir.key())).expect("read the record"),
        Some("2026-11-02T00:00:00Z".to_owned()),
        "the record belongs to another key and stays"
    );
}

#[test]
fn an_absent_key_lets_go_of_a_matching_sidecar() {
    let dir = TempDir::new("identity-orphan-match");
    write_key(&dir.key(), &[16_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-11-03T00:00:00Z").expect("record");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    fs::remove_file(dir.key()).expect("drop the key file");

    remove_key_holding(&dir.key(), &held, Some("2026-11-03T00:00:00Z"))
        .expect("the orphan record still describes the archived key");

    assert!(
        !dir.join(CREATED_NAME).exists(),
        "the matching orphan record is let go of"
    );
}

#[test]
fn an_absent_key_leaves_a_sidecar_that_names_another_key() {
    let dir = TempDir::new("identity-orphan-other");
    write_key(&dir.key(), &[17_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-11-04T00:00:00Z").expect("record the archived key");
    let (_key, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");
    fs::remove_file(dir.key()).expect("drop the key file");
    write_created(Some(&dir.key()), "2026-11-05T00:00:00Z").expect("record the replacement");

    remove_key_holding(&dir.key(), &held, Some("2026-11-04T00:00:00Z"))
        .expect("an absent key is not a failure");

    assert_eq!(
        read_created(Some(&dir.key())).expect("read the record"),
        Some("2026-11-05T00:00:00Z".to_owned()),
        "the record belongs to another key and stays"
    );
}

// -------------------------------------------------------- explicit removal

#[test]
fn removal_drops_the_key_file_and_its_sidecar() {
    let dir = TempDir::new("remove");
    write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-10-01T00:00:00Z").expect("record");

    remove_entry(&dir.key()).expect("remove");

    assert!(!dir.key().exists());
    assert!(!dir.join(CREATED_NAME).exists());
}

#[test]
fn removing_what_is_not_there_succeeds() {
    let dir = TempDir::new("remove-absent");

    remove_entry(&dir.key()).expect("deleting nothing is the state the caller asked for");
}

#[test]
fn a_bare_path_with_no_directory_still_removes_cleanly() {
    // The sidecar path is `None` for a bare filename, so the removal skips it and
    // the file itself is a NotFound, which is the state the caller asked for.
    let bare = format!("clio-sync-key-absent-{}.key", std::process::id());

    remove_entry(std::path::Path::new(&bare)).expect("an absent bare path is not an error");
}

#[test]
fn a_removal_that_cannot_finish_names_the_path_it_stuck_on() {
    let dir = TempDir::new("remove-blocked");
    write_key(&dir.key(), &[8_u8; KEY_LEN]).expect("write");
    // A directory where the sidecar goes: `remove_file` refuses it, and the key
    // is already gone, so the error has to name what was left behind.
    fs::create_dir(dir.join(CREATED_NAME)).expect("the sidecar path is a directory");

    let err = remove_entry(&dir.key()).expect_err("a directory is not removable as a file");

    assert!(err.message.contains("cannot remove"), "{}", err.message);
    assert!(err.message.contains(CREATED_NAME), "{}", err.message);
}
