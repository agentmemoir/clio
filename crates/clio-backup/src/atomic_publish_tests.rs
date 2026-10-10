// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the atomic publish: that a finished
//! file appears under its final name, that an unfinished one leaves nothing, and
//! that the destination directory is flushed after the rename.
//!
//! ## Owns
//! - The commit, drop, and mode assertions over `AtomicFile`
//!
//! ## Does not own
//! - Staging a member and digesting it (see `staging_tests`)
//! - Create, verify, or inventory cases (see their own test modules)
//!
//! ## Boundary
//!
//! Keep this file focused on publishing a finished file.

use super::*;
use crate::fixtures::Scratch;
use std::io::Write;
use std::path::PathBuf;

#[test]
fn an_uncommitted_atomic_file_leaves_no_archive() {
    let scratch = Scratch::new("atomic-uncommitted");
    let final_path = scratch.join("snapshot.cliobak.tar.gz");
    let mut atomic = AtomicFile::reserve(final_path.clone(), "archive").expect("reserve");
    let temp = atomic.temp_path().to_path_buf();

    {
        let mut file = atomic.take_writer().expect("create the temporary file");
        file.write_all(b"partial archive").expect("write");
        assert!(temp.exists(), "the temporary file is where the bytes land");
        assert!(
            !final_path.exists(),
            "no archive is visible before the rename"
        );
    }
    drop(atomic);

    assert!(!temp.exists(), "an uncommitted temporary file is removed");
    assert!(!final_path.exists());
}

#[test]
fn a_committed_atomic_file_appears_under_its_final_name() {
    let scratch = Scratch::new("atomic-commit");
    let final_path = scratch.join("snapshot.cliobak.tar.gz");
    let mut atomic = AtomicFile::reserve(final_path.clone(), "archive").expect("reserve");
    let temp = atomic.temp_path().to_path_buf();

    let mut file = atomic.take_writer().expect("create the temporary file");
    file.write_all(b"complete archive").expect("write");
    let published = atomic.commit(file).expect("commit");

    assert_eq!(published, final_path);
    assert!(final_path.exists());
    assert!(
        !temp.exists(),
        "the temporary name is gone after the rename"
    );
    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        b"complete archive".to_vec()
    );
}

#[cfg(unix)]
#[test]
fn a_published_archive_keeps_the_owner_only_mode() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("atomic-mode");
    let final_path = scratch.join("snapshot.cliobak.tar.gz");
    let mut atomic = AtomicFile::reserve(final_path, "archive").expect("reserve");
    let file = atomic.take_writer().expect("create");
    atomic.commit(file).expect("commit");

    let mode = std::fs::metadata(scratch.join("snapshot.cliobak.tar.gz"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the rename preserves the mode");
}

#[test]
fn a_final_path_without_a_directory_is_refused() {
    let error = AtomicFile::reserve(PathBuf::from("snapshot.cliobak.tar.gz"), "archive")
        .expect_err("a bare file name has no staging directory");

    assert_eq!(error.code, clio_types::ErrorCode::InvalidArgument);
}

/// A commit flushes the destination directory, so a rename into it survives a
/// crash. The flush is only observable as a syscall, so this asserts the
/// committed archive is intact in a directory that was itself just created —
/// the case where a missing directory flush would lose the entry.
#[test]
fn a_commit_into_a_freshly_made_directory_leaves_a_readable_archive() {
    let scratch = Scratch::new("atomic-dirsync");
    let nested = scratch.join("made-now");
    std::fs::create_dir(&nested).expect("create the destination directory");
    let final_path = nested.join("snapshot.cliobak.tar.gz");
    let mut atomic = AtomicFile::reserve(final_path.clone(), "archive").expect("reserve");

    let mut file = atomic.take_writer().expect("create");
    file.write_all(b"archive bytes").expect("write");
    let published = atomic.commit(file).expect("commit");

    assert_eq!(published, final_path);
    assert_eq!(
        std::fs::read(&published).unwrap(),
        b"archive bytes".to_vec()
    );
    assert!(
        !atomic.temp_path().exists(),
        "the temporary name is gone and stays gone"
    );
}

/// A commit into a destination whose parent is a regular file fails before any
/// directory flush, and leaves no archive behind. The directory-flush failure
/// after a successful rename is not reachable from a test: the rename needs a
/// real directory, so the flush that follows has one.
#[test]
fn a_commit_into_a_path_that_is_not_a_directory_fails_cleanly() {
    let scratch = Scratch::new("atomic-nodir");
    let holder = scratch.join("holder");
    std::fs::write(&holder, b"not a directory").expect("put a file where a directory belongs");
    let final_path = holder.join("snapshot.cliobak.tar.gz");
    let mut atomic = AtomicFile::reserve(final_path.clone(), "archive").expect("reserve");
    let error = atomic
        .take_writer()
        .expect_err("a file cannot hold the temporary archive");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(
        !final_path.exists(),
        "no archive appears when the staging directory is not a directory"
    );
}
