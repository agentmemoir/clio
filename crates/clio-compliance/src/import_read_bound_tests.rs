// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bundle source read bound: a source over
//! the byte cap is refused with a message that names no size, a filesystem,
//! type, or UTF-8 failure is refused opaquely, a source that is not JSON says so
//! without its parse detail, and a source at the cap still reads.
//!
//! ## Owns
//! - The three source refusals and their messages, all `InvalidArgument`
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

/// The message every filesystem, type, or UTF-8 failure is refused with.
const UNREADABLE: &str = "bundle source is not a readable bundle file";

/// The message a source over the byte cap is refused with.
const OVERSIZE: &str = "bundle source exceeds the size limit";

/// The message a source whose text is not JSON is refused with.
const NOT_JSON: &str = "bundle JSON invalid";

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

/// Assert a refusal carries the invalid-argument code and exactly `message`.
fn assert_refused(refusal: &(ErrorCode, String), message: &str) {
    assert_eq!(refusal.0, ErrorCode::InvalidArgument, "{refusal:?}");
    assert_eq!(refusal.1, message);
}

/// Assert a message carries no digit and no unit, so it names no size, no
/// maximum, and no parse offset.
fn assert_no_number(message: &str) {
    assert!(
        !message.chars().any(|c| c.is_ascii_digit()),
        "the message names no number: {message}"
    );
    for unit in ["MB", "MiB", "KB", "KiB"] {
        assert!(
            !message.contains(unit),
            "the message names no {unit}: {message}"
        );
    }
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
fn oversize_source_is_refused_without_naming_a_size() {
    let h = Harness::new();
    let source = oversize_source("am-import-oversize");

    let refusal = refusal(&h, source.path());
    assert_refused(&refusal, OVERSIZE);
    assert_no_number(&refusal.1);
    assert!(
        !refusal.1.contains(&MAX_BUNDLE_BYTES.to_string()),
        "the message names no cap: {refusal:?}"
    );
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

    assert_refused(&refusal(&h, source.path()), OVERSIZE);
}

#[test]
fn non_json_source_is_an_invalid_json_refusal() {
    let h = Harness::new();
    let junk = write_source("am-import-junk", b"not json {");

    let refusal = refusal(&h, junk.path());
    assert_refused(&refusal, NOT_JSON);
    assert_no_number(&refusal.1);
    for detail in ["line", "column", "expected", "EOF"] {
        assert!(
            !refusal.1.contains(detail),
            "the message carries no `{detail}`: {refusal:?}"
        );
    }
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
fn cap_splitting_a_multibyte_char_is_an_oversize_refusal() {
    let source = write_source("am-import-split-char", "éééé".as_bytes());

    let err = read_bundle_source(source.path(), 4)
        .expect_err("a source over the cap is refused even when the cap splits a char");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, OVERSIZE);
}

#[test]
fn invalid_utf8_source_is_not_a_readable_bundle_file() {
    let source = write_source("am-import-bad-utf8", &[0xff, 0xfe]);

    let err = read_bundle_source(source.path(), 8).expect_err("invalid UTF-8 is refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, UNREADABLE);
}

/// A missing file, a directory, and a file that is not UTF-8 all report one
/// message and one code, so a caller cannot use the refusal as a
/// filesystem-metadata oracle.
#[test]
fn filesystem_failures_are_indistinguishable() {
    let h = Harness::new();
    let missing = std::env::temp_dir().join(unique("am-import-opaque-missing"));
    let dir = TempDir(std::env::temp_dir().join(unique("am-import-opaque-dir")));
    std::fs::create_dir(&dir.0).unwrap();
    let not_utf8 = write_source("am-import-opaque-utf8", &[0xff, 0xfe]);

    let missing = refusal(&h, missing.to_str().unwrap());
    for (label, other) in [
        ("directory", refusal(&h, dir.0.to_str().unwrap())),
        ("invalid UTF-8", refusal(&h, not_utf8.path())),
    ] {
        assert_eq!(
            other, missing,
            "a {label} source is refused differently from a missing file"
        );
    }
    assert_refused(&missing, UNREADABLE);
}
