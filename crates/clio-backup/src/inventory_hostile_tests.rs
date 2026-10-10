// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hostile archives a snapshot inventory
//! listing must survive: an empty archive, a manifest-only archive, garbage
//! where a member must start, a member with its NUL padding missing, and a
//! directory entry where a regular file must be.
//!
//! ## Owns
//! - The raw-tar hostile fixtures and the T11 listing cases that use them
//!
//! ## Does not own
//! - Ordinary listing, sorting, and read-cost cases (see `inventory_tests`)
//! - Reader or header refusal rules (see `container_reader`, `tar_header`)
//!
//! ## Boundary
//!
//! Changes to what a listing must survive should normally require changes
//! here. Changes to how a member is read should not require changes here.

use super::tests::{MANIFEST_JSON, gzip, raw_member, temp_dir};
use crate::container::ArchiveReader;
use crate::inventory::{list_snapshots, read_manifest};
use crate::manifest::BackupManifest;
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// The header the writer would build for the manifest member, so a patched
/// block differs from a written archive only in the field under test.
fn manifest_block(payload_len: usize) -> [u8; HEADER_SIZE] {
    build_header(crate::MANIFEST_MEMBER, payload_len as u64).expect("header for the manifest")
}

/// Recompute the ustar checksum of a patched header block, as the writer
/// would, so only the field under test is invalid.
fn fix_checksum(block: &mut [u8; HEADER_SIZE]) {
    let sum: u64 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(b)
            }
        })
        .sum();
    let digits = format!("{sum:06o}");
    block[148..154].copy_from_slice(digits.as_bytes());
    block[154] = 0;
    block[155] = b' ';
}

/// Write one raw tar stream as a snapshot file and return the path.
fn write_raw(dir: &Path, tag: &str, raw: &[u8]) -> PathBuf {
    let path = dir.join(format!("{tag}.cliobak.tar.gz"));
    std::fs::write(&path, gzip(raw)).expect("write archive");
    path
}

/// The manifest member followed by the end-of-archive marker.
fn manifest_only_raw() -> Vec<u8> {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend([0u8; 2 * HEADER_SIZE]);
    raw
}

/// Consume the manifest member, then return the error the next member
/// produces, so a damage past the manifest still surfaces.
fn second_member_error(archive: &[u8]) -> AmError {
    let mut r = ArchiveReader::new(archive).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    r.next_member().expect_err("the damaged member is refused")
}

/// List a directory holding exactly one archive, then clean it up.
fn listed_manifest(dir: &Path) -> Option<BackupManifest> {
    let entries = list_snapshots(dir).expect("listing succeeds");
    assert_eq!(entries.len(), 1, "one archive in the scratch directory");
    let manifest = entries[0].manifest.clone();
    let _ = std::fs::remove_dir_all(dir);
    manifest
}

/// A valid empty tar archive: the two zero blocks of the end-of-archive
/// marker and nothing else. `read_manifest` must refuse it with a typed
/// error, so the entry still lists with `manifest: None` and the process is
/// never aborted.
#[test]
fn t11k_empty_archive_lists_without_aborting() {
    let dir = temp_dir("empty");
    let path = write_raw(&dir, "empty", &[0u8; 2 * HEADER_SIZE]);

    let err = read_manifest(&path).expect_err("an empty archive has no manifest");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("no manifest member"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("the archive is empty"),
        "{}",
        err.message
    );
    assert!(
        listed_manifest(&dir).is_none(),
        "an empty archive lists with None"
    );
}

/// An archive carrying the manifest member and no payload still reads, and
/// still lists, with its manifest.
#[test]
fn t11l_manifest_only_archive_lists_with_its_manifest() {
    let dir = temp_dir("manifest-only");
    let path = write_raw(&dir, "manifest-only", &manifest_only_raw());

    let manifest = read_manifest(&path).expect("a manifest-only archive reads");
    assert_eq!(manifest.kind, crate::manifest::KIND);
    assert!(listed_manifest(&dir).is_some());
}

/// Trailing garbage after a clean end-of-archive marker: the manifest still
/// reads, because the manifest is the first member and nothing else is read.
#[test]
fn t11m_trailing_garbage_after_the_end_marker_still_reads_the_manifest() {
    let dir = temp_dir("trailing");
    let mut raw = manifest_only_raw();
    raw.extend(vec![0xABu8; 1024]);
    let path = write_raw(&dir, "trailing", &raw);

    let manifest = read_manifest(&path).expect("the manifest itself reads cleanly");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(manifest.kind, crate::manifest::KIND);
}

/// Garbage where the manifest member must start: the reader refuses it, so
/// the archive lists with `manifest: None` rather than aborting.
#[test]
fn t11n_garbage_where_the_manifest_member_must_start_lists_with_none() {
    let dir = temp_dir("garbage-first");
    let raw = vec![0xABu8; HEADER_SIZE];
    let path = write_raw(&dir, "garbage-first", &raw);

    let err = read_manifest(&path).expect_err("garbage is not a manifest member");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
}

/// A manifest member whose NUL padding to the 512-byte boundary is missing.
/// A manifest read discards the padding region without parsing it, so the
/// manifest still reads and the archive still lists; a full read of the same
/// archive is refused, which is the damage the missing padding really is.
#[test]
fn t11o_manifest_without_its_nul_padding_lists_without_aborting() {
    let payload_len = MANIFEST_JSON.len();
    let pad = (HEADER_SIZE - payload_len % HEADER_SIZE) % HEADER_SIZE;
    assert!(pad > 0, "the manifest JSON is not a whole number of blocks");
    let mut raw = manifest_block(payload_len).to_vec();
    raw.extend_from_slice(MANIFEST_JSON.as_bytes());
    let next_header = build_header("store.db", 4).expect("header");
    // Shift the second member's header left by the padding it needed.
    raw.extend_from_slice(&next_header[pad..]);
    raw.extend_from_slice(b"data");
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let dir = temp_dir("nopadding");
    let path = write_raw(&dir, "no-padding", &raw);
    let gz = std::fs::read(&path).expect("read the archive back");

    let manifest = read_manifest(&path).expect("the manifest region itself is intact");
    assert_eq!(manifest.kind, crate::manifest::KIND);
    assert!(listed_manifest(&dir).is_some());

    let err = second_member_error(&gz);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
}

/// A directory entry (typeflag `5`) where the manifest member must be: the
/// archive carries only regular files, so it is refused and lists with
/// `manifest: None`.
#[test]
fn t11p_directory_entry_where_the_manifest_member_must_be_lists_with_none() {
    let dir = temp_dir("dirname");
    let mut header = manifest_block(MANIFEST_JSON.len());
    header[156] = b'5';
    fix_checksum(&mut header);
    let mut raw = header.to_vec();
    raw.extend_from_slice(MANIFEST_JSON.as_bytes());
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let path = write_raw(&dir, "directory-entry", &raw);

    let err = read_manifest(&path).expect_err("a directory member is refused");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("unsupported member type '5'"),
        "{}",
        err.message
    );
}

/// Every hostile archive in this file survives a whole listing: one broken
/// file never hides the rest of the directory and never aborts.
#[test]
fn t11q_hostile_archives_never_hide_the_valid_snapshots_around_them() {
    let dir = temp_dir("hostile-mix");
    let valid = gzip(&manifest_only_raw());
    std::fs::write(dir.join("a-valid.cliobak.tar.gz"), &valid).expect("write valid");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("b-empty", vec![0u8; 2 * HEADER_SIZE]),
        ("c-garbage-first", vec![0xABu8; HEADER_SIZE]),
    ];
    for (name, raw) in cases {
        write_raw(&dir, name, &raw);
    }
    let entries = list_snapshots(&dir).expect("listing succeeds");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(entries.len(), 3, "every archive still lists");
    assert_eq!(
        entries[0].path.file_name().and_then(|n| n.to_str()),
        Some("a-valid.cliobak.tar.gz")
    );
    assert!(entries[0].manifest.is_some(), "the valid archive is intact");
    for entry in &entries[1..] {
        assert!(
            entry.manifest.is_none(),
            "{} must list with None",
            entry.path.display()
        );
    }
}

/// A gzip file that is not an archive at all is still a listing entry, not a
/// failure of the listing: the corrupt-file case must stay total.
#[test]
fn t11r_non_gzip_file_lists_with_none() {
    let dir = temp_dir("not-gzip");
    std::fs::write(dir.join("junk.cliobak.tar.gz"), b"not gzip at all").expect("write junk file");

    assert!(listed_manifest(&dir).is_none());
}
