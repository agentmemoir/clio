// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the owner-only journal file tests: the
//! durable, exact-mode write, the stale-temp removal, and the raw read
//! classification.
//!
//! ## Owns
//! - The write, expected-pre-image guard, and read assertions for the journal
//!   file layer
//!
//! ## Does not own
//! - The journal model, schema, and operations (see `erasure_journal_tests.rs`)
//! - The file mechanics under test
//!
//! ## Boundary
//!
//! Tests here break when the journal's atomic write sequence, temp naming, or
//! raw read classification changes.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the journal's file mechanics.

use super::{ExpectedBody, MAX_JOURNAL_BYTES, ReadBody, read_body, write_body};
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};

/// A unique scratch directory removed on drop.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-compliance-erasure-journal-file"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    fn journal(&self) -> PathBuf {
        self.root.join("erasure-journal.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The deterministic same-directory temp sibling of `path`.
fn temp_of(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap();
    path.with_file_name(format!(".{name}.erasure-journal-tmp"))
}

/// Seed a raw file and restrict it to the owner where mode bits exist, so the
/// size and body classifications are not masked by a widened mode.
fn write_owner_only(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("seed the raw file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the seeded file");
    }
}

/// An oversized body one byte over the limit.
fn oversized() -> Vec<u8> {
    vec![b'x'; usize::try_from(MAX_JOURNAL_BYTES).expect("a usize limit") + 1]
}

#[test]
fn write_body_creates_a_missing_parent_directory() {
    let scratch = Scratch::new();
    // A first denial under a fresh data directory writes through a parent
    // that does not exist yet, so the write creates it instead of refusing.
    let path = scratch.root.join("fresh-dir").join("erasure-journal.json");
    write_body(&path, &ExpectedBody::Absent, b"body").expect("a missing parent is created");
    assert_eq!(std::fs::read(&path).expect("read back"), b"body");
}

#[test]
fn write_body_writes_the_exact_bytes_owner_only() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    write_body(&path, &ExpectedBody::Absent, b"journal-bytes").expect("write the body");
    assert_eq!(std::fs::read(&path).expect("read back"), b"journal-bytes");
    match read_body(&path) {
        ReadBody::Bytes(bytes) => assert_eq!(bytes, b"journal-bytes"),
        _ => panic!("the written body is no longer readable"),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the journal is owner-only from its first byte"
        );
    }
}

#[test]
fn write_body_replaces_the_target_and_removes_a_stale_temp() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let temp = temp_of(&path);
    write_owner_only(&path, b"old");
    std::fs::write(&temp, b"stale").expect("seed the stale temp");
    write_body(&path, &ExpectedBody::Bytes(b"old"), b"new").expect("replace the body");
    assert_eq!(std::fs::read(&path).expect("read back"), b"new");
    assert!(!temp.exists(), "the stale temp is removed");
}

#[test]
fn write_body_never_replaces_a_directory_at_the_temp_name() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let temp = temp_of(&path);
    std::fs::create_dir(&temp).expect("occupy the temp name with a directory");
    let err = write_body(&path, &ExpectedBody::Absent, b"body")
        .expect_err("a directory at the temp name refuses");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        temp.is_dir(),
        "the directory at the temp name is never removed"
    );
    assert!(
        !path.exists(),
        "no journal is written through a blocked temp"
    );
}

#[test]
fn read_body_classifies_absent_damaged_and_bytes() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    assert!(matches!(read_body(&path), ReadBody::Absent));
    write_owner_only(&path, b"");
    assert!(
        matches!(read_body(&path), ReadBody::Damaged),
        "an empty body is damaged"
    );
    write_owner_only(&path, &oversized());
    assert!(
        matches!(read_body(&path), ReadBody::Damaged),
        "an oversized body is damaged"
    );
    std::fs::remove_file(&path).expect("clear the damaged body");
    write_body(&path, &ExpectedBody::Absent, b"body").expect("write a bounded body");
    assert!(
        matches!(read_body(&path), ReadBody::Bytes(_)),
        "a bounded body reads"
    );
}

#[cfg(unix)]
#[test]
fn read_body_refuses_a_symlink_at_the_journal_path() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let path = scratch.journal();
    let target = scratch.root.join("owner-only-target.json");
    write_owner_only(&target, b"body");
    assert!(
        matches!(read_body(&target), ReadBody::Bytes(_)),
        "the symlink target itself is a valid owner-only body"
    );
    symlink(&target, &path).expect("plant the symlink at the journal path");
    assert!(
        matches!(read_body(&path), ReadBody::Damaged),
        "a symlink at the journal path is damaged even when its target reads"
    );
    // The link itself is neither followed nor replaced.
    assert_eq!(std::fs::read_link(&path).expect("the link"), target);
}

#[test]
fn write_body_refuses_an_oversized_body() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let err = write_body(&path, &ExpectedBody::Absent, &oversized())
        .expect_err("an oversized body refuses");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{}", err.message);
    assert!(!path.exists(), "nothing is written for an oversized body");
}

#[cfg(unix)]
#[test]
fn read_body_refuses_a_group_or_other_readable_file() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let path = scratch.journal();
    std::fs::write(&path, b"body").expect("seed the file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("widen the mode");
    assert!(
        matches!(read_body(&path), ReadBody::Damaged),
        "a widened journal is damaged"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("restrict again");
    assert!(
        matches!(read_body(&path), ReadBody::Bytes(_)),
        "the same bytes read once they are owner-only"
    );
}

#[test]
fn write_body_refuses_a_destination_that_changed_under_it() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    write_body(&path, &ExpectedBody::Absent, b"first").expect("seed the body");
    write_body(&path, &ExpectedBody::Bytes(b"first"), b"newer").expect("the matching write lands");
    assert_eq!(std::fs::read(&path).expect("read back"), b"newer");
    let err = write_body(&path, &ExpectedBody::Bytes(b"first"), b"stale")
        .expect_err("a stale pre-image refuses");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    let kept = std::fs::read(&path).expect("read back");
    assert_eq!(kept, b"newer", "the newer destination bytes survive");
    assert!(!temp_of(&path).exists(), "no temp is left behind");
}

#[test]
fn write_body_refuses_an_absent_pre_image_once_the_destination_exists() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    write_body(&path, &ExpectedBody::Absent, b"first").expect("seed the body");
    let err = write_body(&path, &ExpectedBody::Absent, b"second")
        .expect_err("an appeared destination refuses");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    let kept = std::fs::read(&path).expect("read back");
    assert_eq!(kept, b"first", "the first writer's bytes survive");
    assert!(!temp_of(&path).exists(), "no temp is left behind");
}

#[cfg(unix)]
#[test]
fn write_body_refuses_a_symlink_at_the_temp_name() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let path = scratch.journal();
    let temp = temp_of(&path);
    let target = scratch.root.join("target.json");
    write_owner_only(&target, b"target");
    symlink(&target, &temp).expect("plant the symlink at the temp name");
    let err = write_body(&path, &ExpectedBody::Absent, b"body")
        .expect_err("a symlink at the temp name refuses");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    let link = std::fs::symlink_metadata(&temp).expect("the link is never removed");
    assert!(link.file_type().is_symlink(), "the link is still a link");
    assert_eq!(std::fs::read_link(&temp).expect("the link"), target);
    let kept = std::fs::read(&target).expect("read the target");
    assert_eq!(kept, b"target", "the link is never followed");
    assert!(
        !path.exists(),
        "no journal is written through a blocked temp"
    );
}

#[test]
fn write_body_accepts_a_body_of_exactly_the_limit() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let body = vec![b'x'; usize::try_from(MAX_JOURNAL_BYTES).expect("a usize limit")];
    write_body(&path, &ExpectedBody::Absent, &body).expect("the limit itself writes");
    match read_body(&path) {
        ReadBody::Bytes(bytes) => assert_eq!(bytes, body, "the limit itself reads"),
        _ => panic!("the exact limit is not skipped by the size gate"),
    }
}

#[test]
fn read_body_refuses_a_directory_at_the_journal_path() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    std::fs::create_dir(&path).expect("occupy the journal path with a directory");
    assert!(
        matches!(read_body(&path), ReadBody::Damaged),
        "a directory at the journal path is damaged"
    );
    assert!(path.is_dir(), "the directory is never replaced");
}
