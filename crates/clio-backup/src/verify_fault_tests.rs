// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the verify pass's fault cases: a member that
//! ends before its header promised, a key member the manifest never declared,
//! and a directory that cannot hold the extracted payload.
//!
//! ## Owns
//! - The short-member and undeclared-member refusals
//! - The refusal when the staging directory cannot be written
//!
//! ## Does not own
//! - The digest, presence, and integrity verdicts (see `verify_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on how a verify pass fails.

use super::*;
use crate::container::ArchiveReader;
use crate::fixtures::{Scratch, pad, request, staged_payload};
use crate::tar_header::{HEADER_SIZE, build_header};
use flate2::write::GzEncoder;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

/// A stub integrity check that always passes, so the archive half is exercised
/// without a database.
#[allow(clippy::unnecessary_wraps)]
fn always_ok(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

/// Create a keyless archive and return its path.
fn keyless_archive(scratch: &Scratch) -> PathBuf {
    let payload = staged_payload(scratch.path(), 44);
    let req = request(scratch.path(), payload);
    crate::create::create_archive(&req).expect("create").path
}

/// An archive whose `member` header declares far more bytes than it carries,
/// and then ends.
///
/// The container opens and the manifest reads, so the member read is what
/// fails — the case a whole-stream truncation never reaches.
fn archive_with_short_member(scratch: &Scratch, member: &str, payload: &[u8]) -> PathBuf {
    let file = std::fs::File::open(keyless_archive(scratch)).expect("open the source archive");
    let mut reader = ArchiveReader::new(file).expect("read the source archive");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    drop(reader);
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&pad(&manifest)).unwrap();
    encoder
        .write_all(&build_header(member, payload.len() as u64 + 4096).unwrap())
        .unwrap();
    encoder.write_all(payload).unwrap();
    let path = scratch.join("short.cliobak.tar.gz");
    std::fs::write(&path, encoder.finish().expect("finish")).expect("write");
    path
}

#[test]
fn a_payload_member_that_ends_early_is_refused() {
    let scratch = Scratch::new("verify-short-payload");
    let path = archive_with_short_member(&scratch, "store.db", b"partial payload");

    let error = verify_archive(&path, &mut always_ok).expect_err("a short payload is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(error.message.contains("store.db"), "{}", error.message);
    let temporaries: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("verify-payload"))
        .collect();
    assert!(temporaries.is_empty(), "the partial copy was removed");
}

#[test]
fn a_key_member_that_ends_early_is_refused() {
    let scratch = Scratch::new("verify-short-keys");
    let path = archive_with_short_member(&scratch, crate::KEY_MEMBER, b"SQLite format 3\0ab");

    let error = verify_archive(&path, &mut always_ok).expect_err("a short key member is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(crate::KEY_MEMBER),
        "{}",
        error.message
    );
}

#[test]
fn an_unlisted_member_that_ends_early_is_refused() {
    let scratch = Scratch::new("verify-short-unlisted");
    let path = archive_with_short_member(&scratch, "dump/items.txt", b"items\n");

    let error = verify_archive(&path, &mut always_ok).expect_err("a short member is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains("dump/items.txt"),
        "{}",
        error.message
    );
}

#[test]
fn a_key_member_the_manifest_does_not_declare_is_refused() {
    let scratch = Scratch::new("verify-extra-keys");
    let path = keyless_archive(&scratch);
    // The manifest says no keys, so a key member in the archive is a lie.
    let file = std::fs::File::open(&path).expect("open");
    let mut reader = ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    let mut payload = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut payload).unwrap();
    let keys = crate::fixtures::SQLITE_MAGIC.to_vec();
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&pad(&manifest)).unwrap();
    encoder
        .write_all(&build_header("store.db", payload.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&payload).unwrap();
    encoder.write_all(&pad(&payload)).unwrap();
    encoder
        .write_all(&build_header(crate::KEY_MEMBER, keys.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&keys).unwrap();
    encoder.write_all(&pad(&keys)).unwrap();
    encoder.write_all(&[0u8; 2 * HEADER_SIZE]).unwrap();
    std::fs::write(&path, encoder.finish().expect("finish")).expect("write");

    let result = verify_archive(&path, &mut always_ok).expect("the manifest still reads");

    assert!(!result.ok, "an undeclared key member must fail");
    assert!(result.failed().contains(&crate::KEY_MEMBER));
}

/// An archive whose payload header declares `declared` bytes over the manifest's
/// payload. The stream stops after the real payload, so the header is the only
/// place the lie appears.
fn archive_with_payload_header(scratch: &Scratch, declared: u64) -> PathBuf {
    use crate::verify::verify_hostile_manifest_tests::{PAYLOAD, manifest_bytes};

    let manifest = manifest_bytes(PAYLOAD, false, None);
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&pad(&manifest)).unwrap();
    encoder.write_all(&build_header("store.db", declared).unwrap()).unwrap();
    encoder.write_all(PAYLOAD).unwrap();
    encoder.write_all(&pad(PAYLOAD)).unwrap();
    encoder.write_all(&[0u8; 2 * HEADER_SIZE]).unwrap();
    let path = scratch.join("size-lie.cliobak.tar.gz");
    std::fs::write(&path, encoder.finish().expect("finish")).expect("write");
    path
}

/// The scratch directory holds no staged payload, so a refusal wrote nothing.
fn no_staged_payload(scratch: &Scratch) {
    let temporaries: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("verify-payload"))
        .collect();
    assert!(temporaries.is_empty(), "no payload was staged: {temporaries:?}");
}

#[test]
fn a_payload_header_size_the_manifest_does_not_record_is_refused_before_staging() {
    let scratch = Scratch::new("verify-size-lie");
    let declared = crate::verify::verify_hostile_manifest_tests::PAYLOAD.len() as u64 + 4096;
    let path = archive_with_payload_header(&scratch, declared);

    let error = verify_archive_with_space(
        &path,
        &mut always_ok,
        &|_, _| panic!("the space check must not run for a header-size lie"),
    )
    .expect_err("a header size the manifest does not record is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(error.message.contains("store.db"), "{}", error.message);
    assert!(error.message.contains(&declared.to_string()), "{}", error.message);
    assert!(
        error.message.contains(&crate::verify::verify_hostile_manifest_tests::PAYLOAD.len().to_string()),
        "{}",
        error.message
    );
    no_staged_payload(&scratch);
}

#[test]
fn a_payload_that_does_not_fit_is_refused_before_staging() {
    let scratch = Scratch::new("verify-no-room");
    let path = keyless_archive(&scratch);

    let error = verify_archive_with_space(&path, &mut always_ok, &|_, bytes| {
        Err(AmError::new(
            ErrorCode::QuotaExceeded,
            format!("{bytes} bytes do not fit"),
        ))
    })
    .expect_err("a payload that does not fit is refused");

    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    no_staged_payload(&scratch);
}

#[cfg(unix)]
#[test]
fn a_directory_that_cannot_hold_the_staged_copy_fails_the_verification() {
    // Root ignores mode bits, so the case cannot be produced there.
    if std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .map(|uids| uids.split_whitespace().next() == Some("0"))
        })
        .unwrap_or(false)
    {
        return;
    }
    let scratch = Scratch::new("verify-unwritable");
    let path = keyless_archive(&scratch);
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o500))
        .expect("chmod read-only");

    let result = verify_archive(&path, &mut always_ok);

    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700))
        .expect("restore");
    let error = result.expect_err("a directory that cannot hold the copy must fail");
    // Regression cover for issue #83: this used to be asserted as
    // `ArchiveCorrupt`, which told the operator their archive was damaged when
    // the archive was fine and the directory was the problem. A staging fault
    // keeps the code it arrived with, so it is never "the archive is damaged".
    assert_ne!(
        error.code,
        ErrorCode::ArchiveCorrupt,
        "a machine fault must not be reported as a damaged archive: {}",
        error.message
    );
    assert!(error.message.contains("store.db"), "{}", error.message);
    // The member is still named, so the operator knows which member stopped.
    assert!(
        error.message.contains("staged"),
        "the message must say what was being done: {}",
        error.message
    );
}
