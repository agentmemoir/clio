// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bundle source read bound: a source over
//! the byte cap is refused before parsing, a non-regular source is refused
//! before the read, and a source at the cap still reads.
//!
//! ## Owns
//! - The size-limit refusal and its error code
//! - The non-regular-source refusal
//! - The missing-source message and the read at or under the cap
//!
//! ## Does not own
//! - Bundle parsing or checksum validation (see `import_tests.rs`)
//! - Fixture builders (see `import_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::path::PathBuf;

use clio_types::ErrorCode;

use crate::import::{MAX_BUNDLE_BYTES, import, read_bundle_source};
use crate::import_fixture::{Harness, TempBundle, unique};

/// A temporary directory removed on drop, including during a panic unwind.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A temporary source file removed on drop.
fn write_source(name: &str, bytes: &[u8]) -> TempBundle {
    let path = std::env::temp_dir().join(unique(name));
    std::fs::write(&path, bytes).unwrap();
    TempBundle(path)
}

#[test]
fn oversize_source_is_refused_before_parsing() {
    let h = Harness::new();
    // A sparse file gives a real source over the cap without writing 10 MiB.
    let path = std::env::temp_dir().join(unique("am-import-oversize"));
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_BUNDLE_BYTES + 1).unwrap();
    drop(file);
    let source = TempBundle(path);

    let err = import(&h.args(&h.ctx(), h.policy(), source.path(), true, false, None))
        .expect_err("a source over the cap is refused");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("byte limit"), "{err:?}");
    assert!(
        !err.message.contains("JSON invalid"),
        "the source is refused before parsing: {err:?}"
    );
}

#[test]
fn directory_source_is_refused_as_non_regular() {
    let h = Harness::new();
    let dir = TempDir(std::env::temp_dir().join(unique("am-import-dir")));
    std::fs::create_dir(&dir.0).unwrap();

    let err = import(&h.args(
        &h.ctx(),
        h.policy(),
        dir.0.to_str().unwrap(),
        true,
        false,
        None,
    ))
    .expect_err("a directory is not a readable bundle");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("not a regular file"), "{err:?}");
}

#[test]
fn source_at_or_under_the_cap_is_still_read() {
    let at_cap = write_source("am-import-at-cap", b"12345678");
    assert_eq!(
        read_bundle_source(at_cap.path(), 8).unwrap(),
        "12345678",
        "a source exactly at the cap reads"
    );

    let under = write_source("am-import-under-cap", b"1234567");
    assert_eq!(
        read_bundle_source(under.path(), 8).unwrap(),
        "1234567",
        "a source under the cap reads"
    );
}

#[test]
fn cap_splitting_a_multibyte_char_still_reports_the_size_limit() {
    let source = write_source("am-import-split-char", "éééé".as_bytes());

    let err = read_bundle_source(source.path(), 4)
        .expect_err("a source over the cap is refused even when the cap splits a char");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("byte limit"), "{err:?}");
}

#[test]
fn invalid_utf8_source_is_refused_as_invalid_argument() {
    let source = write_source("am-import-bad-utf8", &[0xff, 0xfe]);

    let err = read_bundle_source(source.path(), 8).expect_err("invalid UTF-8 is refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("invalid UTF-8"), "{err:?}");
}

#[test]
fn missing_source_still_reports_bundle_read_failed() {
    let h = Harness::new();
    let missing = std::env::temp_dir().join(unique("am-import-missing"));

    let err = import(&h.args(
        &h.ctx(),
        h.policy(),
        missing.to_str().unwrap(),
        true,
        false,
        None,
    ))
    .expect_err("a missing source is refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("bundle read failed"), "{err:?}");
}

/// A source whose reported size is over the cap is refused from metadata, so
/// neither the open nor the read runs: the file carries no read permission, and
/// a refusal reached through the open reports `bundle read failed` instead.
/// A process that could read it anyway still gets the same size-limit refusal
/// from the post-read bound, so the test does not depend on the permission.
#[cfg(unix)]
#[test]
fn oversize_reported_size_is_refused_before_the_read() {
    use std::os::unix::fs::PermissionsExt as _;

    let path = std::env::temp_dir().join(unique("am-import-oversize-meta"));
    // A sparse file: the reported size is over the cap, the disk cost is not.
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_BUNDLE_BYTES + 1).unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o000))
        .unwrap();
    drop(file);
    let source = TempBundle(path);

    let err = read_bundle_source(source.path(), MAX_BUNDLE_BYTES)
        .expect_err("a reported size over the cap is refused");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("byte limit"), "{err:?}");
    assert!(
        !err.message.contains("bundle read failed"),
        "the source is refused before the open and the read: {err:?}"
    );
}
