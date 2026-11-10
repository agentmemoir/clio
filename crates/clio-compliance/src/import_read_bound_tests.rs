// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bundle source read bound: a source over
//! the byte cap is refused, a non-regular source is refused before the read, a
//! source at the cap still reads, and every one of those refusals is the same
//! opaque error so a caller cannot tell the reasons apart.
//!
//! ## Owns
//! - The single unreadable-source refusal, its message, and its error code
//! - The read at or under the cap
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

/// The one message every unreadable bundle source is refused with.
const UNREADABLE: &str = "bundle source is not a readable bundle file";

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

/// The refusal `import` reports for `source`, as its code and message.
fn refusal(h: &Harness, source: &str) -> (ErrorCode, String) {
    let err = import(&h.args(&h.ctx(), h.policy(), source, true, false, None))
        .expect_err("an unreadable source is refused");
    (err.code, err.message)
}

/// Assert a refusal is the opaque one, carrying no hint of its branch.
fn assert_opaque(refusal: &(ErrorCode, String)) {
    assert_eq!(refusal.0, ErrorCode::InvalidArgument, "{refusal:?}");
    assert_eq!(refusal.1, UNREADABLE);
}

/// A sparse file whose reported size is over the cap, at no disk cost.
fn oversize_source(name: &str) -> TempBundle {
    let path = std::env::temp_dir().join(unique(name));
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_BUNDLE_BYTES + 1).unwrap();
    drop(file);
    TempBundle(path)
}

#[test]
fn oversize_source_is_refused() {
    let h = Harness::new();
    let source = oversize_source("am-import-oversize");

    let refusal = refusal(&h, source.path());
    assert_opaque(&refusal);
}

#[test]
fn directory_source_is_refused() {
    let h = Harness::new();
    let dir = TempDir(std::env::temp_dir().join(unique("am-import-dir")));
    std::fs::create_dir(&dir.0).unwrap();

    let refusal = refusal(&h, dir.0.to_str().unwrap());
    assert_opaque(&refusal);
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
fn cap_splitting_a_multibyte_char_is_refused() {
    let source = write_source("am-import-split-char", "éééé".as_bytes());

    let err = read_bundle_source(source.path(), 4)
        .expect_err("a source over the cap is refused even when the cap splits a char");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, UNREADABLE);
}

#[test]
fn invalid_utf8_source_is_refused() {
    let source = write_source("am-import-bad-utf8", &[0xff, 0xfe]);

    let err = read_bundle_source(source.path(), 8).expect_err("invalid UTF-8 is refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, UNREADABLE);
}

#[test]
fn missing_source_is_refused() {
    let h = Harness::new();
    let missing = std::env::temp_dir().join(unique("am-import-missing"));

    let refusal = refusal(&h, missing.to_str().unwrap());
    assert_opaque(&refusal);
}

/// A missing file, a directory, an oversize file, and a file that is not JSON
/// all report one message and one code, so a caller cannot use the refusal as a
/// filesystem-metadata oracle.
#[test]
fn unreadable_bundle_sources_are_indistinguishable() {
    let h = Harness::new();
    let missing = std::env::temp_dir().join(unique("am-import-opaque-missing"));
    let dir = TempDir(std::env::temp_dir().join(unique("am-import-opaque-dir")));
    std::fs::create_dir(&dir.0).unwrap();
    let oversize = oversize_source("am-import-opaque-oversize");
    let junk = write_source("am-import-opaque-junk", b"not json");

    let missing = refusal(&h, missing.to_str().unwrap());
    for (label, refusal) in [
        ("directory", refusal(&h, dir.0.to_str().unwrap())),
        ("oversize", refusal(&h, oversize.path())),
        ("non-JSON", refusal(&h, junk.path())),
    ] {
        assert_eq!(
            refusal, missing,
            "a {label} source is refused differently from a missing file"
        );
    }
    assert_opaque(&missing);
}

/// The refusal is reached from the reported size, so it holds whether or not the
/// process could open the file.
#[cfg(unix)]
#[test]
fn oversize_source_without_read_permission_is_still_refused() {
    use std::os::unix::fs::PermissionsExt as _;

    let h = Harness::new();
    let source = oversize_source("am-import-oversize-meta");
    std::fs::set_permissions(source.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

    let refusal = refusal(&h, source.path());
    assert_opaque(&refusal);
}
