// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the archive container itself: round
//! trips, determinism, reader bounds, reader discipline, writer-order
//! enforcement, and writer-side refusals.
//!
//! ## Owns
//! - The T01/T03/T07/T14 container cases that need no second opinion
//!
//! ## Does not own
//! - The shared fixtures (see `container_fixtures_tests`)
//! - External-reader interop, corruption, or io-failure cases (see the
//!   sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Keep this file focused on `container` behavior.

use super::fixtures_tests::{
    EMPTY_SHA256, MANIFEST_JSON, gzip, raw_member, read_all_members, sha_hex, standard_members,
    write_full_archive,
};
use super::{ArchiveReader, ArchiveWriter, ReaderBounds, declared_size_suspect};
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::ErrorCode;

/// T01: the four-member archive round-trips byte-exactly, digests match,
/// and the manifest is member 0.
#[test]
fn t01_four_member_round_trip() {
    let archive = write_full_archive();
    let members = read_all_members(&archive);

    assert_eq!(members[0].0, crate::MANIFEST_MEMBER, "manifest is member 0");
    assert_eq!(members[0].1, MANIFEST_JSON.as_bytes());

    let expected = standard_members();
    assert_eq!(members.len(), expected.len() + 1);
    for (got, (name, bytes)) in members[1..].iter().zip(expected) {
        assert_eq!(got.0, name);
        assert_eq!(got.1, bytes, "member {name} round-trips byte-exactly");
        assert_eq!(got.2.size, bytes.len() as u64);
        assert_eq!(got.2.sha256, sha_hex(&bytes), "digest of {name}");
        assert_eq!(got.2.sha256.len(), 64);
    }
}

/// T03c: a zero-length member is legal and round-trips.
#[test]
fn t03c_zero_length_member_round_trip() {
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let digest = w.add_member("empty.bin", &b""[..]).expect("empty member");
    let archive = w.finish().expect("finish");

    assert_eq!(digest.size, 0);
    assert_eq!(digest.sha256, EMPTY_SHA256);

    let members = read_all_members(&archive);
    assert_eq!(members.len(), 2);
    assert_eq!(members[1].0, "empty.bin");
    assert!(members[1].1.is_empty(), "{:?}", members[1].1);
    assert_eq!(members[1].2.sha256, EMPTY_SHA256);
}

/// The whole archive is byte-identical when built twice.
#[test]
fn t22_whole_archive_is_byte_identical() {
    assert_eq!(write_full_archive(), write_full_archive());
}

/// Each reader bound is enforced with its own distinct message.
#[test]
fn t07b_reader_bounds_enforced() {
    // Member count: a third member is refused.
    let archive = write_full_archive();
    let bounds = ReaderBounds {
        max_members: 2,
        max_member_bytes: u64::MAX,
        max_total_bytes: u64::MAX,
    };
    let mut r = ArchiveReader::with_bounds(archive.as_slice(), bounds).expect("opens");
    for _ in 0..2 {
        r.next_member().expect("header within the count cap");
        r.skip_current().expect("payload within the count cap");
    }
    let err = r.next_member().expect_err("third member refused");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(err.message.contains("cap of 2 members"), "{}", err.message);
    let count_message = err.message;

    // Per-member size: the refusal happens in next_member, before any
    // payload byte exists in the stream.
    let mut raw = raw_member(crate::MANIFEST_MEMBER, MANIFEST_JSON.as_bytes());
    raw.extend(
        build_header("store.db", 1024 * 1024)
            .expect("header")
            .to_vec(),
    );
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let bounds = ReaderBounds {
        max_members: 10,
        max_member_bytes: 1024,
        max_total_bytes: u64::MAX,
    };
    let gz = gzip(&raw);
    let mut r = ArchiveReader::with_bounds(gz.as_slice(), bounds).expect("opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let err = r
        .next_member()
        .expect_err("oversized declared member refused");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message
            .contains("over the per-member cap of 1024 bytes"),
        "{}",
        err.message
    );
    assert_ne!(err.message, count_message);

    // Total size: two members whose combined payload crosses the cap are
    // refused mid-stream, while each member alone stays under it.
    let manifest_len = MANIFEST_JSON.len() as u64;
    let pad = |len: u64| (HEADER_SIZE as u64 - len % HEADER_SIZE as u64) % HEADER_SIZE as u64;
    let after_manifest = manifest_len + pad(manifest_len);
    let member_bytes = 1000u64;
    let after_one = after_manifest + member_bytes + pad(member_bytes);
    let bounds = ReaderBounds {
        max_members: 1024,
        max_member_bytes: u64::MAX,
        max_total_bytes: after_one + 500,
    };
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    w.add_member(
        "one.db",
        std::io::Cursor::new(vec![1u8; member_bytes as usize]),
    )
    .expect("member one");
    w.add_member(
        "two.db",
        std::io::Cursor::new(vec![2u8; member_bytes as usize]),
    )
    .expect("member two");
    let archive = w.finish().expect("finish");
    let mut r = ArchiveReader::with_bounds(archive.as_slice(), bounds).expect("opens");
    r.next_member().expect("manifest header");
    r.read_current(&mut Vec::new()).expect("manifest fits");
    r.next_member().expect("one.db header");
    r.skip_current().expect("one.db fits under the total cap");
    r.next_member().expect("two.db header");
    let err = r
        .read_current(&mut Vec::new())
        .expect_err("two.db crosses the total cap mid-stream");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains("total inflated-size cap"),
        "{}",
        err.message
    );
    assert_ne!(err.message, count_message);
}

/// `declared_size_suspect` is true only when the declared size is
/// strictly larger than the recorded size.
#[test]
fn t07c_declared_size_suspect() {
    assert!(!declared_size_suspect(100, 100));
    assert!(!declared_size_suspect(100, 101));
    assert!(declared_size_suspect(101, 100));
}

/// A reader cap is a property of the archive, never of the machine.
///
/// Freeing disk space cannot raise a cap, so a cap that arrived as
/// `QuotaExceeded` sent the operator to the wrong place: exit 1 and a free-space
/// hint for an archive no filesystem can make readable. The code is pinned here
/// so the two cannot drift back together, and `QuotaExceeded` is left to mean
/// what a caller reads as "the machine has no room".
#[test]
fn t07d_a_reader_cap_is_not_the_machines_quota() {
    let archive = write_full_archive();
    let bounds = ReaderBounds {
        max_members: 1,
        max_member_bytes: u64::MAX,
        max_total_bytes: u64::MAX,
    };
    let mut r = ArchiveReader::with_bounds(archive.as_slice(), bounds).expect("opens");
    r.next_member()
        .expect("the one member within the count cap");
    r.skip_current().expect("its payload within the count cap");
    let err = r
        .next_member()
        .expect_err("a second member is over the cap");

    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert_ne!(
        err.code,
        ErrorCode::QuotaExceeded,
        "a cap is this archive, not this machine: {}",
        err.message
    );
}

/// The writer enforces member order: manifest first, exactly once, and the
/// manifest member name is reserved.
#[test]
fn t14_writer_order_enforced() {
    let mut w = ArchiveWriter::new(Vec::new());
    let err = w
        .add_member("store.db", &b"x"[..])
        .expect_err("no manifest yet");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    assert!(
        err.message
            .contains("the manifest must be the first member"),
        "{}",
        err.message
    );

    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let err = w
        .write_manifest(MANIFEST_JSON.as_bytes())
        .expect_err("second manifest refused");
    assert!(
        err.message.contains("already been written"),
        "{}",
        err.message
    );

    let err = w
        .add_member(crate::MANIFEST_MEMBER, &b"x"[..])
        .expect_err("reserved name refused");
    assert!(err.message.contains("reserved"), "{}", err.message);

    w.add_member("store.db", &b"x"[..]).expect("legal member");
    w.finish().expect("finish");
}

/// Reader discipline: no member is open before next_member, an open member
/// must be consumed first, and a clean archive ends with Ok(None).
#[test]
fn t15_reader_discipline() {
    let archive = write_full_archive();
    let mut r = ArchiveReader::new(archive.as_slice()).expect("opens");

    let mut sink = Vec::new();
    let err = r
        .read_current(&mut sink)
        .expect_err("no member is open yet");
    assert!(err.message.contains("no member is open"), "{}", err.message);
    let err = r.skip_current().expect_err("no member is open yet");
    assert!(err.message.contains("no member is open"), "{}", err.message);

    r.next_member().expect("manifest header");
    let err = r.next_member().expect_err("member still open");
    assert!(err.message.contains("still open"), "{}", err.message);
    r.skip_current().expect("manifest payload");

    let mut seen = 0;
    while r.next_member().expect("next member").is_some() {
        r.skip_current().expect("skip member");
        seen += 1;
    }
    assert_eq!(seen, 3);
    assert!(r.next_member().expect("after the end marker").is_none());
}

/// An illegal member name is refused by the name gate and nothing is
/// written: the writer still reports "manifest first" for the next call, so
/// the refused call never reached a header.
#[test]
fn t19_illegal_member_name_writes_nothing() {
    let mut w = ArchiveWriter::new(Vec::new());
    let err = w
        .add_member("../escape", &b"x"[..])
        .expect_err("the name gate refuses the name");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    assert!(
        err.message.contains("invalid archive member name"),
        "{}",
        err.message
    );
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let digest = w.add_member("store.db", &b"x"[..]).expect("legal member");
    assert_eq!(digest.size, 1, "the refused call wrote nothing");
}
