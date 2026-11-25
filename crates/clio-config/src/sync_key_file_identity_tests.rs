// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the descriptor a sync key read returns:
//! its bytes are the file's bytes, an absent path holds nothing, and the
//! missing-file refusal a plain read reports is unchanged.
//!
//! ## Owns
//! - Every test of `read_key_holding`, and the unchanged `read_key` refusal
//!
//! ## Does not own
//! - Which entry a removal may unlink (see `sync_key_file_remove_tests`)
//! - Reading, creating, and the sidecar's rules (see `sync_key_file_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a
//! unique temp directory removed when the guard drops, and no test waits, sleeps,
//! or starts a thread.
//!
//! Keep this module focused on the read side of the held descriptor.

use super::*;
use crate::sync_key_test_support::TempDir;

#[test]
fn a_held_read_returns_the_bytes_of_the_file_it_opened() {
    let dir = TempDir::new("identity-bytes");
    let key = [13_u8; KEY_LEN];
    write_key(&dir.key(), &key).expect("write");

    let (read, held) = read_key_holding(&dir.key())
        .expect("read")
        .expect("a key to hold");

    assert_eq!(*read, key, "the bytes are the file's bytes");
    assert_eq!(
        held.metadata().expect("metadata").len(),
        KEY_LEN as u64,
        "the descriptor is the key file itself"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held_meta = held.metadata().expect("held metadata");
        let path_meta = std::fs::metadata(dir.key()).expect("path metadata");
        assert_eq!(
            (held_meta.dev(), held_meta.ino()),
            (path_meta.dev(), path_meta.ino()),
            "the descriptor is open on the file the path still names"
        );
    }
}

#[test]
fn reading_an_absent_key_file_holds_nothing() {
    let dir = TempDir::new("identity-none");

    let held = read_key_holding(&dir.key()).expect("an absent file is not a failure");

    assert!(held.is_none(), "nothing at the path is nothing to hold");
}

#[test]
fn an_absent_key_file_keeps_its_missing_message() {
    let dir = TempDir::new("identity-missing");

    let err = read_key(&dir.key()).expect_err("nothing to read");

    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert_eq!(
        err.message,
        format!(
            "cannot read the Clio sync key file {}: the file does not exist. Clio will not \
             generate a replacement, because a new key cannot open anything sealed under the old \
             one; restore access to the file and run again",
            dir.key().display()
        ),
        "a caller that expected a key still gets the same refusal"
    );
}
