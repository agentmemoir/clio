// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing archive verification: a good
//! archive passing, a flipped payload byte failing, a damaged or truncated
//! archive failing with a specific message, a refused manifest, and the
//! integrity check being called on the extracted payload.
//!
//! ## Owns
//! - The verify-path assertions, including the stub integrity check
//!
//! ## Does not own
//! - The store's real integrity check (see `clio-store`)
//! - Create and inventory cases (see their own test modules)
//!
//! ## Boundary
//!
//! Keep this file focused on `verify` behavior.

use super::*;
use crate::container::ArchiveReader;
use crate::fixtures::{Scratch, key_source, request, staged_payload};
use crate::tar_header::{HEADER_SIZE, build_header};
use flate2::write::GzEncoder;
use std::io::Write;
use std::path::Path;

/// A stub integrity check that always passes, standing in for the store's own
/// check so the archive half is tested without a database.
#[allow(clippy::unnecessary_wraps)]
fn always_ok(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

/// A stub integrity check that always fails, for the report path.
fn always_fails(path: &Path) -> Result<(), AmError> {
    Err(AmError::new(
        ErrorCode::ArchiveCorrupt,
        format!("stub integrity failure for {}", path.display()),
    ))
}

/// Create a good archive and return its report.
fn good_archive(scratch: &Scratch, with_keys: bool) -> (crate::create::CreateReport, PathBuf) {
    let payload = staged_payload(scratch.path(), 20);
    let mut req = request(scratch.path(), payload);
    if with_keys {
        req.keys = Some(key_source(scratch.path(), 2));
    }
    let report = crate::create::create_archive(&req).expect("create");
    let path = report.path.clone();
    (report, path)
}

#[test]
fn a_good_archive_passes_every_member() {
    let scratch = Scratch::new("verify-ok");
    let (report, path) = good_archive(&scratch, true);

    let result = verify_archive(&path, &mut always_ok).expect("verify");

    assert!(result.ok, "failed checks: {:?}", result.failed());
    let failed = result.failed();
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(result.id, report.id);
    assert_eq!(result.backend, "sqlite");
    assert!(result.counts_complete);
    assert!(result.unlisted.is_empty(), "{:?}", result.unlisted);
    assert_eq!(
        result.members.len(),
        4,
        "manifest, payload, payload integrity, and the key member"
    );
    assert!(
        result
            .members
            .iter()
            .any(|check| check.detail.contains("both match the manifest"))
    );
}

#[test]
fn a_keyless_archive_reports_the_key_member_absent_and_passes() {
    let scratch = Scratch::new("verify-keyless");
    let (_, path) = good_archive(&scratch, false);

    let result = verify_archive(&path, &mut always_ok).expect("verify");

    assert!(result.ok, "failed checks: {:?}", result.failed());
    assert!(
        !result
            .members
            .iter()
            .any(|check| check.member == KEY_MEMBER)
    );
}

#[test]
fn a_failing_store_integrity_check_fails_the_report() {
    let scratch = Scratch::new("verify-integrity");
    let (_, path) = good_archive(&scratch, false);

    let result = verify_archive(&path, &mut always_fails).expect("verify");

    assert!(!result.ok);
    assert!(
        result
            .failed()
            .iter()
            .any(|name| name.contains("integrity")),
        "the integrity failure is reported: {:?}",
        result.failed()
    );
}

#[test]
fn a_flipped_payload_byte_is_caught_by_the_digest() {
    let scratch = Scratch::new("verify-flip");
    let (_, path) = good_archive(&scratch, false);
    // Flip one byte inside the compressed stream, which the gzip decoder
    // usually reports as a corrupt stream rather than bad content; either way
    // the archive must not verify.
    let mut bytes = std::fs::read(&path).expect("read the archive");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    std::fs::write(&path, &bytes).expect("write the damaged archive");

    let error = verify_archive(&path, &mut always_ok).expect_err("a damaged archive is refused");
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);

    // A payload whose *content* changed but whose container stays valid is the
    // case the digest exists for: rebuild the archive with one member byte
    // altered and a manifest that still claims the old digest.
    let scratch = Scratch::new("verify-digest");
    let (_, path) = good_archive(&scratch, false);
    rewrite_payload_byte(&path);
    let result = verify_archive(&path, &mut always_ok).expect("the container still reads");
    assert!(!result.ok, "a changed payload byte must fail");
    assert!(
        result.failed().contains(&"store.db"),
        "the payload member is named: {:?}",
        result.failed()
    );
}

/// Rewrite the archive with one payload byte changed and the manifest untouched.
fn rewrite_payload_byte(path: &Path) {
    let file = std::fs::File::open(path).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    let mut manifest = Vec::new();
    assert!(reader.next_member().expect("manifest").is_some());
    reader.read_current(&mut manifest).expect("manifest bytes");
    let mut payload = Vec::new();
    assert!(reader.next_member().expect("payload").is_some());
    reader.read_current(&mut payload).expect("payload bytes");
    assert!(reader.next_member().expect("end of archive").is_none());
    payload[0] = payload[0].wrapping_add(1);
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(
            &crate::tar_header::build_header(crate::MANIFEST_MEMBER, manifest.len() as u64)
                .expect("manifest header"),
        )
        .expect("write");
    encoder.write_all(&manifest).expect("write");
    encoder
        .write_all(&crate::fixtures::pad(&manifest))
        .expect("pad");
    encoder
        .write_all(&build_header("store.db", payload.len() as u64).expect("payload header"))
        .expect("write");
    encoder.write_all(&payload).expect("write");
    encoder
        .write_all(&crate::fixtures::pad(&payload))
        .expect("pad");
    encoder
        .write_all(&[0u8; 2 * HEADER_SIZE])
        .expect("end marker");
    let bytes = encoder.finish().expect("finish");
    std::fs::write(path, bytes).expect("write the rebuilt archive");
}

#[test]
fn a_truncated_archive_is_refused_with_a_specific_message() {
    let scratch = Scratch::new("verify-truncated");
    let (_, path) = good_archive(&scratch, false);
    let bytes = std::fs::read(&path).expect("read the archive");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate");

    let error = verify_archive(&path, &mut always_ok).expect_err("a truncated archive is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(!error.message.is_empty(), "the refusal names what is wrong");
}

#[test]
fn an_unknown_format_version_is_refused() {
    let scratch = Scratch::new("verify-version");
    let (_, path) = good_archive(&scratch, false);
    rewrite_manifest_field(&path, "\"format_version\":1", "\"format_version\":99");

    let error = verify_archive(&path, &mut always_ok).expect_err("an unknown version is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains("format_version"),
        "{}",
        error.message
    );
}

#[test]
fn a_wrong_kind_is_refused() {
    let scratch = Scratch::new("verify-kind");
    let (_, path) = good_archive(&scratch, false);
    rewrite_manifest_field(
        &path,
        "\"kind\":\"clio-store-backup\"",
        "\"kind\":\"something-else\"",
    );

    let error = verify_archive(&path, &mut always_ok).expect_err("a wrong kind is refused");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(error.message.contains("kind"), "{}", error.message);
}

/// Rebuild the archive with one manifest string replaced.
fn rewrite_manifest_field(path: &Path, from: &str, to: &str) {
    let file = std::fs::File::open(path).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    let mut manifest = Vec::new();
    assert!(reader.next_member().expect("manifest").is_some());
    reader.read_current(&mut manifest).expect("manifest bytes");
    let mut payload = Vec::new();
    assert!(reader.next_member().expect("payload").is_some());
    reader.read_current(&mut payload).expect("payload bytes");
    let text = String::from_utf8(manifest).expect("manifest is utf-8");
    let rewritten = text
        .replace(from, to)
        .replace(&text, &text.replace(from, to));
    let manifest = rewritten.into_bytes();
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).expect("header"))
        .expect("write");
    encoder.write_all(&manifest).expect("write");
    encoder
        .write_all(&crate::fixtures::pad(&manifest))
        .expect("pad");
    encoder
        .write_all(&build_header("store.db", payload.len() as u64).expect("header"))
        .expect("write");
    encoder.write_all(&payload).expect("write");
    encoder
        .write_all(&crate::fixtures::pad(&payload))
        .expect("pad");
    encoder
        .write_all(&[0u8; 2 * HEADER_SIZE])
        .expect("end marker");
    std::fs::write(path, encoder.finish().expect("finish")).expect("write");
}

#[test]
fn a_missing_archive_is_refused() {
    let scratch = Scratch::new("verify-absent");
    let error = verify_archive(&scratch.join("nothing.cliobak.tar.gz"), &mut always_ok)
        .expect_err("an absent archive cannot be verified");

    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn a_missing_payload_member_is_named() {
    let scratch = Scratch::new("verify-no-payload");
    let (_, path) = good_archive(&scratch, false);
    // Keep only the manifest member.
    let file = std::fs::File::open(&path).expect("open");
    let mut reader = ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&crate::fixtures::pad(&manifest)).unwrap();
    encoder.write_all(&[0u8; 2 * HEADER_SIZE]).unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).expect("write");

    let result = verify_archive(&path, &mut always_ok).expect("the manifest still reads");

    assert!(!result.ok);
    assert!(
        result.failed().contains(&"store.db"),
        "the absent payload is named: {:?}",
        result.failed()
    );
}

#[test]
fn a_manifest_that_claims_keys_without_the_member_is_named() {
    let scratch = Scratch::new("verify-missing-keys");
    let payload = staged_payload(scratch.path(), 21);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 1));
    let report = crate::create::create_archive(&req).expect("create");
    // Drop the key member, leaving the manifest's claim in place.
    let file = std::fs::File::open(&report.path).expect("open");
    let mut reader = ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    let mut payload_bytes = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut payload_bytes).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&crate::fixtures::pad(&manifest)).unwrap();
    encoder
        .write_all(&build_header("store.db", payload_bytes.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&payload_bytes).unwrap();
    encoder
        .write_all(&crate::fixtures::pad(&payload_bytes))
        .unwrap();
    encoder.write_all(&[0u8; 2 * HEADER_SIZE]).unwrap();
    std::fs::write(&report.path, encoder.finish().unwrap()).expect("write");

    let result = verify_archive(&report.path, &mut always_ok).expect("the manifest still reads");

    assert!(!result.ok);
    assert!(result.failed().contains(&KEY_MEMBER));
}

#[test]
fn an_unlisted_member_is_listed_and_not_treated_as_a_pass() {
    let scratch = Scratch::new("verify-unlisted");
    let (_, path) = good_archive(&scratch, false);
    let file = std::fs::File::open(&path).expect("open");
    let mut reader = ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    let mut payload = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut payload).unwrap();
    let extra = b"dump/items.txt";
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&build_header(crate::MANIFEST_MEMBER, manifest.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&manifest).unwrap();
    encoder.write_all(&crate::fixtures::pad(&manifest)).unwrap();
    encoder
        .write_all(&build_header("store.db", payload.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&payload).unwrap();
    encoder.write_all(&crate::fixtures::pad(&payload)).unwrap();
    encoder
        .write_all(&build_header("dump/items.txt", extra.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(extra).unwrap();
    encoder.write_all(&crate::fixtures::pad(extra)).unwrap();
    encoder.write_all(&[0u8; 2 * HEADER_SIZE]).unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).expect("write");

    let result = verify_archive(&path, &mut always_ok).expect("verify");

    assert_eq!(result.unlisted, vec!["dump/items.txt".to_owned()]);
    assert!(
        result
            .members
            .iter()
            .any(|check| check.member == "dump/items.txt" && check.detail.contains("not trusted")),
        "the unlisted member says it is not trusted"
    );
}

#[test]
fn a_key_member_that_is_not_a_key_store_database_is_refused() {
    let scratch = Scratch::new("verify-bad-keys");
    let payload = staged_payload(scratch.path(), 22);
    let mut req = request(scratch.path(), payload);
    let key = key_source(scratch.path(), 1);
    // A file that is not a SQLite key store must not pass as one.
    std::fs::write(&key.path, b"{\"not\":\"a database\"}").expect("write a bad key store");
    req.keys = Some(key);
    let report = crate::create::create_archive(&req).expect("create");

    let result = verify_archive(&report.path, &mut always_ok).expect("verify");

    assert!(
        !result.ok,
        "a key member that is not a key-store database must fail"
    );
    assert!(result.failed().contains(&KEY_MEMBER));
}

#[test]
fn a_key_member_whose_size_disagrees_with_the_manifest_fails() {
    let scratch = Scratch::new("verify-keys-size");
    let (_, path) = good_archive(&scratch, true);
    let manifest = crate::inventory::read_manifest(&path).expect("manifest");
    let recorded = manifest.keys.bytes.expect("the manifest records");
    let mut reader = ArchiveReader::new(std::fs::File::open(&path).expect("open")).expect("reader");
    loop {
        let next = reader.next_member().expect("next");
        if next.expect("a key member exists").name == KEY_MEMBER {
            break;
        }
        reader.skip_current().expect("skip");
    }
    let check = crate::verify_members::key_check(
        &mut reader,
        manifest.keys.carries_key_member(),
        Some(recorded + 1),
    )
    .expect("key check");
    assert!(!check.ok, "{}", check.detail);
    let d = &check.detail;
    assert!(d.contains("the manifest records"), "{}", d);
    assert!(d.contains("the member holds"), "{}", d);
}

#[test]
fn the_extracted_payload_never_survives_the_verification() {
    let scratch = Scratch::new("verify-cleanup");
    let (_, path) = good_archive(&scratch, false);

    verify_archive(&path, &mut always_ok).expect("verify");

    let temporaries: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("clio-snapshot-verify-payload"))
        .collect();
    assert!(
        temporaries.is_empty(),
        "the extracted payload was left behind: {temporaries:?}"
    );
}
