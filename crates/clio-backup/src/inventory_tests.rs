// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the snapshot inventory: listing,
//! sorting, ignoring non-snapshots, per-archive manifest reads, the
//! manifest-first read-cost property, and the hostile archives an inventory
//! listing must survive.
//!
//! ## Owns
//! - The inventory test fixtures (directory cases and raw archive builder)
//! - The T11 listing cases for ordinary, valid archives
//!
//! ## Does not own
//! - Hostile-archive listing cases (see `inventory_hostile_tests`)
//! - Container or manifest tests (see the sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Keep this file focused on `inventory` behavior.

use super::*;
use crate::container::ArchiveWriter;
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::ErrorCode;
use flate2::write::GzEncoder;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so this helper never hands two
/// callers the same path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A valid manifest, as JSON. Shared with the hostile-archive cases.
pub(super) const MANIFEST_JSON: &str = r#"{"kind":"clio-store-backup","format_version":1,"created_at":"2026-09-27T00:00:00Z","tool_version":"clio 0.1.0","store":{"backend":"postgres","slug":"memoir","database":"clio","schema":"public"},"snapshot":{"method":"offline-copy","consistency":"point-in-time","started_at":"2026-09-27T00:00:00Z","duration_ms":42},"payload":{"member":"store.db","bytes":64,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","uncompressed_bytes":64},"counts":{"complete":false},"schema_version":"1","keys":{"included":true,"member":"keys.kms.db","subjects":3,"bytes":15,"note":"key store copied; per-subject keys are wrapped and the Clio key is not included"},"label":"nightly","compression":"gzip"}"#;

/// One raw ustar member: header, payload, NUL padding. Shared with the
/// hostile-archive cases.
pub(super) fn raw_member(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, payload.len() as u64)
        .expect("header")
        .to_vec();
    out.extend_from_slice(payload);
    let pad = (HEADER_SIZE - payload.len() % HEADER_SIZE) % HEADER_SIZE;
    out.extend(vec![0u8; pad]);
    out
}

/// Compress raw tar bytes. Shared with the hostile-archive cases.
pub(super) fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).expect("gzip write");
    e.finish().expect("gzip finish")
}

/// A standard valid archive: manifest plus one payload member.
fn sample_archive() -> Vec<u8> {
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    w.add_member("store.db", b"SQLite format 3\0".as_slice())
        .expect("payload member");
    w.finish().expect("finish")
}

/// A unique scratch directory under the system temp directory. Shared with
/// the hostile-archive cases.
pub(super) fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is set")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "clio-backup-inv-{tag}-{}-{nanos}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// T11: matching files list sorted by name; non-matching files and
/// subdirectories are ignored.
#[test]
fn t11_list_snapshots_sorted_and_ignores_others() {
    let dir = temp_dir("sorted");
    for (name, bytes) in [
        ("b-20260927-aabbccdd.cliobak.tar.gz", sample_archive()),
        ("a-20260926-11223344.cliobak.tar.gz", sample_archive()),
        ("notes.txt", b"not a snapshot".to_vec()),
        ("mid.cliobak.tar", b"wrong suffix".to_vec()),
        ("a-20260925-wrongext.cliobak.tgz", b"wrong suffix".to_vec()),
    ] {
        std::fs::write(dir.join(name), bytes).expect("write snapshot file");
    }
    std::fs::create_dir(dir.join("z-20260925-00000000.cliobak.tar.gz"))
        .expect("create decoy subdirectory");

    let entries = list_snapshots(&dir).expect("listing succeeds");
    let _ = std::fs::remove_dir_all(&dir);

    let names: Vec<String> = entries
        .iter()
        .map(|e| {
            e.path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "a-20260926-11223344.cliobak.tar.gz".to_string(),
            "b-20260927-aabbccdd.cliobak.tar.gz".to_string()
        ]
    );
    for entry in &entries {
        let manifest = entry
            .manifest
            .as_ref()
            .expect("valid archives list a manifest");
        assert_eq!(manifest.kind, crate::manifest::KIND);
    }
}

/// A missing directory is a NotFound error, not an empty listing.
#[test]
fn t11b_missing_directory_is_not_found() {
    let dir = temp_dir("missing");
    let err = list_snapshots(&dir.join("absent")).expect_err("no such directory");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::NotFound, "{}", err.message);
    assert!(
        err.message.contains("no backup directory"),
        "{}",
        err.message
    );
}

/// An unreadable archive lists with `manifest: None` and never hides the
/// valid snapshots around it.
#[test]
fn t11c_corrupt_archive_lists_with_none() {
    let dir = temp_dir("corrupt");
    std::fs::write(dir.join("good-1.cliobak.tar.gz"), sample_archive())
        .expect("write valid archive");
    std::fs::write(dir.join("bad-1.cliobak.tar.gz"), b"definitely not gzip")
        .expect("write corrupt file");

    let entries = list_snapshots(&dir).expect("listing succeeds");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0].path.file_name().and_then(|n| n.to_str()),
        Some("bad-1.cliobak.tar.gz"),
        "listing is sorted by file name"
    );
    assert!(
        entries[0].manifest.is_none(),
        "the corrupt file lists with None"
    );
    assert_eq!(
        entries[1].path.file_name().and_then(|n| n.to_str()),
        Some("good-1.cliobak.tar.gz")
    );
    assert!(
        entries[1].manifest.is_some(),
        "the valid file lists a manifest"
    );
}

/// `read_manifest` refuses an archive whose first member is not the
/// manifest (the container enforces the rule; inventory surfaces it).
#[test]
fn t11d_read_manifest_refuses_non_manifest_first_member() {
    let dir = temp_dir("firstmember");
    let mut raw = raw_member("store.db", b"payload first, manifest never");
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let path = dir.join("wrong-first.cliobak.tar.gz");
    std::fs::write(&path, gzip(&raw)).expect("write archive");

    let err = read_manifest(&path).expect_err("a non-manifest first member is refused");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("the first archive member must be"),
        "{}",
        err.message
    );
}

/// A first member declaring more than `MAX_MANIFEST_BYTES` is refused
/// before its bytes are read.
#[test]
fn t11e_oversized_manifest_refused() {
    let dir = temp_dir("bigmanifest");
    let path = dir.join("big-manifest.cliobak.tar.gz");
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(&vec![b'x'; 100 * 1024])
        .expect("the writer has no manifest cap");
    let archive = w.finish().expect("finish");
    std::fs::write(&path, archive).expect("write archive");

    let err = read_manifest(&path).expect_err("a 100 KiB manifest member is refused");
    let _ = std::fs::remove_dir_all(&dir);
    // The reader's cap is a property of the archive, so it arrives as the code
    // that means "this file is not readable by this build" and never as the
    // machine's `quota_exceeded`.
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(err.message.contains("per-member cap"), "{}", err.message);
}

/// The read-cost proof: the payload member exists only in the raw tar and
/// the file is cut right after the manifest member plus the start of the
/// next header, so a full-stream read would fail on truncation.
/// `read_manifest` succeeds anyway: it never inflates the payload.
#[test]
fn t11f_truncated_after_manifest_still_lists() {
    let dir = temp_dir("readcost");
    let payload_len = 300 * 1024usize;
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(
        build_header("store.db", payload_len as u64)
            .expect("header")
            .to_vec(),
    );
    raw.extend(vec![0xABu8; payload_len]);
    raw.extend([0u8; 2 * HEADER_SIZE]);

    let manifest_len = MANIFEST_JSON.len();
    let pad = (HEADER_SIZE - manifest_len % HEADER_SIZE) % HEADER_SIZE;
    let cut = HEADER_SIZE + manifest_len + pad + HEADER_SIZE + 1;
    let path = dir.join("cut-1.cliobak.tar.gz");
    std::fs::write(&path, gzip(&raw[..cut])).expect("write truncated archive");

    let manifest = read_manifest(&path).expect("reading the manifest never inflates the payload");
    let entries = list_snapshots(&dir).expect("listing succeeds");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(manifest.kind, crate::manifest::KIND);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].manifest.is_some());
}

/// `read_manifest` reports a missing file as NotFound.
#[test]
fn t11g_read_manifest_missing_file_is_not_found() {
    let dir = temp_dir("nofile");
    let err = read_manifest(&dir.join("absent.cliobak.tar.gz")).expect_err("no such file");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::NotFound, "{}", err.message);
}
/// A file that cannot be opened for a reason other than "not found" (here
/// a symlink loop) is an Internal error, not NotFound.
#[test]
fn t11h_open_error_other_than_not_found() {
    let dir = temp_dir("symlinkloop");
    let a = dir.join("a.cliobak.tar.gz");
    let b = dir.join("b.cliobak.tar.gz");
    std::os::unix::fs::symlink(&b, &a).expect("create symlink");
    std::os::unix::fs::symlink(&a, &b).expect("create symlink");

    let err = read_manifest(&a).expect_err("symlink loop");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(err.message.contains("opening"), "{}", err.message);
}

/// Listing a path that is a file, not a directory, is an Internal error
/// with its own message.
#[test]
fn t11i_read_dir_on_file_is_internal() {
    let dir = temp_dir("notadir");
    let file = dir.join("plain.txt");
    std::fs::write(&file, b"not a directory").expect("write file");

    let err = list_snapshots(&file).expect_err("path is a file");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(err.message.contains("reading"), "{}", err.message);
}

/// A manifest member that declares more bytes than the stream holds is
/// refused with both sizes named.
#[test]
fn t11j_manifest_member_ends_early() {
    let dir = temp_dir("ends-early");
    let mut raw = build_header(crate::MANIFEST_MEMBER, 100)
        .expect("header")
        .to_vec();
    raw.extend_from_slice(b"0123456789");
    let path = dir.join("short.cliobak.tar.gz");
    std::fs::write(&path, gzip(&raw)).expect("write archive");

    let err = read_manifest(&path).expect_err("manifest member ends early");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message
            .contains("declared 100 bytes but the stream ended after 10 bytes"),
        "{}",
        err.message
    );
}
