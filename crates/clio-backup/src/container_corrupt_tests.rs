// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the archive reader's corruption
//! refusals: truncated streams, bad checksums, and broken end-of-archive
//! markers.
//!
//! ## Owns
//! - The T04 corruption cases and the raw-archive builder they read
//!
//! ## Does not own
//! - Round-trip, bounds, or writer-order tests (see `container_tests`)
//! - The shared fixtures (see `container_fixtures_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on refused-to-read archives.

use super::fixtures_tests::{MANIFEST_JSON, gunzip, gzip, raw_member, write_full_archive};
use super::{ArchiveReader, ReaderBounds};
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::{AmError, ErrorCode};

/// T04: a truncated gzip stream is refused as ArchiveCorrupt.
#[test]
fn t04_truncated_gzip_refused() {
    let full = write_full_archive();
    let cut = &full[..full.len() - 100];
    let err = read_until_error(cut);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("truncated or corrupt"),
        "{}",
        err.message
    );
}

/// T04: a flipped byte in the header checksum field is refused.
#[test]
fn t04b_corrupt_header_checksum_refused() {
    let mut raw = gunzip(&write_full_archive());
    raw[148] ^= 0x01; // inside the 8-byte chksum field
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    let err = r.next_member().expect_err("bad checksum refused");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(err.message.contains("checksum mismatch"), "{}", err.message);
}

/// T04: an archive with no end-of-archive marker is refused, which
/// CPython's tarfile would tolerate but this build does not.
#[test]
fn t04c_missing_end_of_archive_marker_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(raw_member("store.db", b"data"));
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    r.next_member().expect("store.db header");
    r.skip_current().expect("store.db payload");
    let err = r.next_member().expect_err("no end marker");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("missing end-of-archive marker"),
        "{}",
        err.message
    );
}

/// T04: one zero block followed by more data, and a partial trailing
/// block, are each refused with their own messages.
#[test]
fn t04d_broken_end_marker_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(raw_member("store.db", b"data"));
    raw.extend([0u8; HEADER_SIZE]);
    raw.extend(raw_member("keys.kms.db", b"keys"));
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    for _ in 0..2 {
        r.next_member().expect("header");
        r.skip_current().expect("payload");
    }
    let err = r
        .next_member()
        .expect_err("data after the first zero block");
    assert!(
        err.message.contains("data after the end-of-archive marker"),
        "{}",
        err.message
    );

    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend([0u8; 300]);
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let err = r.next_member().expect_err("partial trailing block");
    assert!(
        err.message
            .contains("end of stream in the end-of-archive marker"),
        "{}",
        err.message
    );
}

/// A member whose header declares more bytes than the stream holds is
/// refused with the declared and actual sizes named.
#[test]
fn t04e_member_shorter_than_declared_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(build_header("store.db", 10).expect("header").to_vec());
    raw.extend_from_slice(b"abc");
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let header = r.next_member().expect("store.db header").expect("member");
    assert_eq!(header.size, 10);
    let mut sink = Vec::new();
    let err = r.read_current(&mut sink).expect_err("payload ends early");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message
            .contains("declared 10 bytes but the stream ended after 3 bytes"),
        "{}",
        err.message
    );
}

/// Read members until an error surfaces; panic if the stream reads clean.
fn read_until_error(archive: &[u8]) -> AmError {
    let mut r = ArchiveReader::new(archive).expect("gzip opens");
    loop {
        match r.next_member() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("archive read cleanly; corruption expected"),
            Err(e) => return e,
        }
        if let Err(e) = r.skip_current() {
            return e;
        }
    }
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

/// Build a manifest member plus a patched-header member, so the reader
/// gets past the manifest-first rule before the patched header is parsed.
fn archive_with_patched_header(patch: impl FnOnce(&mut [u8; HEADER_SIZE])) -> Vec<u8> {
    let mut header = build_header("store.db", 4).expect("header");
    patch(&mut header);
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(header.to_vec());
    raw.extend_from_slice(b"data");
    raw.extend([0u8; 2 * HEADER_SIZE]);
    gzip(&raw)
}

/// Consume the manifest member, then return the error of the next member.
fn second_member_error(archive: &[u8]) -> AmError {
    let mut r = ArchiveReader::new(archive).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    r.next_member().expect_err("header must be refused")
}

/// T04: a header with a broken magic field is refused before anything else.
#[test]
fn t04f_bad_magic_refused() {
    let archive = archive_with_patched_header(|b| b[257..263].copy_from_slice(b"ustarX"));
    let err = second_member_error(&archive);
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("not a ustar header"),
        "{}",
        err.message
    );
}

/// T04: an unsupported ustar version is refused, naming found and supported.
#[test]
fn t04g_bad_ustar_version_refused() {
    let archive = archive_with_patched_header(|b| b[263..265].copy_from_slice(b"01"));
    let err = second_member_error(&archive);
    assert!(
        err.message.contains("unsupported ustar version"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("\"01\"") && err.message.contains("\"00\""),
        "{}",
        err.message
    );
}

/// T04: a non-file typeflag (a directory entry here) is refused.
#[test]
fn t04h_non_file_typeflag_refused() {
    let archive = archive_with_patched_header(|b| b[156] = b'5');
    let err = second_member_error(&archive);
    assert!(
        err.message.contains("unsupported member type '5'"),
        "{}",
        err.message
    );
}

/// T04: a non-octal checksum field is refused.
#[test]
fn t04i_malformed_checksum_field_refused() {
    let archive = archive_with_patched_header(|b| {
        b[148..156].copy_from_slice(b"zzzz\0\0\0\0");
    });
    let err = second_member_error(&archive);
    assert!(
        err.message.contains("the checksum field is not octal"),
        "{}",
        err.message
    );
}

/// T04: a non-octal or GNU-binary size field is refused.
#[test]
fn t04j_malformed_size_field_refused() {
    let all_nul = archive_with_patched_header(|b| {
        b[124..136].fill(0);
        fix_checksum(b);
    });
    let err = second_member_error(&all_nul);
    assert!(
        err.message.contains("the size field is not octal"),
        "{}",
        err.message
    );

    let binary = archive_with_patched_header(|b| {
        b[124] |= 0x80;
        fix_checksum(b);
    });
    let err = second_member_error(&binary);
    assert!(
        err.message.contains("the size field is not octal"),
        "{}",
        err.message
    );
}

/// T04: an invalid member name in a header is refused by the reader's name
/// gate, before the rest of the header is used.
#[test]
fn t04k_reader_rejects_invalid_member_name() {
    let mut header = build_header("a\\b", 4).expect("header");
    fix_checksum(&mut header);
    let mut raw = header.to_vec();
    raw.extend_from_slice(b"data");
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    let err = r.next_member().expect_err("name gate must fire");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message
            .contains("archive member name is not acceptable"),
        "{}",
        err.message
    );
    assert!(err.message.contains("backslash"), "{}", err.message);
}

/// T04: a member whose padding is missing is refused.
#[test]
fn t04l_missing_padding_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(build_header("store.db", 10).expect("header").to_vec());
    raw.extend_from_slice(b"0123456789");
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    r.next_member().expect("store.db header");
    let mut sink = Vec::new();
    let err = r.read_current(&mut sink).expect_err("padding is missing");
    assert!(
        err.message.contains("end of stream in the padding"),
        "{}",
        err.message
    );
}

/// T04: a partial second zero block of the end marker is refused.
#[test]
fn t04m_partial_second_marker_block_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend([0u8; HEADER_SIZE]);
    raw.extend([0u8; 100]);
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let err = r.next_member().expect_err("second marker block is partial");
    assert!(
        err.message
            .contains("end of stream in the end-of-archive marker"),
        "{}",
        err.message
    );
}

/// T04: a partial trailing block that is not all zero is a truncated
/// member header, with its own message.
#[test]
fn t04n_partial_header_block_refused() {
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(vec![0xEE; 300]);
    let gz = gzip(&raw);
    let mut r = ArchiveReader::new(gz.as_slice()).expect("gzip opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let err = r.next_member().expect_err("partial header block");
    assert!(
        err.message
            .contains("end of stream in a member header block"),
        "{}",
        err.message
    );
}

/// The total-size cap also fires during padding bytes, not only payload
/// bytes.
#[test]
fn t04o_total_cap_during_padding_refused() {
    let manifest_len = MANIFEST_JSON.len() as u64;
    let pad = |len: u64| (HEADER_SIZE as u64 - len % HEADER_SIZE as u64) % HEADER_SIZE as u64;
    let after_manifest = manifest_len + pad(manifest_len);
    let bounds = ReaderBounds {
        max_members: 1024,
        max_member_bytes: u64::MAX,
        max_total_bytes: after_manifest + 1000 + 10,
    };
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(raw_member("store.db", &vec![1u8; 1000]));
    let gz = gzip(&raw);
    let mut r = ArchiveReader::with_bounds(gz.as_slice(), bounds).expect("opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    r.next_member().expect("store.db header");
    let err = r.skip_current().expect_err("padding crosses the total cap");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("total inflated-size cap"),
        "{}",
        err.message
    );
}
