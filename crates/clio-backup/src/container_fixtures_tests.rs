// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the archive container test fixtures every
//! other container test module builds on: the standard archive bytes, the raw
//! ustar member builder, the gzip helpers, and the scratch-path helper.
//!
//! ## Owns
//! - The standard four-member test archive and the manifest JSON it carries
//! - The raw member / gzip / gunzip builders and the hex digest helper
//!
//! ## Does not own
//! - Any assertion: every fixture is data, never a test of its own
//! - Round-trip, bounds, corruption, or interop cases (see the sibling
//!   `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Changes to how the shared fixtures are built should normally require
//! changes here. Adding a case to a sibling test module should not require
//! changes here.

use super::{ArchiveReader, ArchiveWriter, MemberDigest};
use crate::tar_header::{HEADER_SIZE, build_header};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so this helper never hands two
/// callers the same path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A valid manifest, as JSON, with a placeholder digest the reader-side
/// tests never rely on. Shared with the corruption test file.
pub(super) const MANIFEST_JSON: &str = r#"{"kind":"clio-store-backup","format_version":1,"created_at":"2026-09-27T00:00:00Z","tool_version":"clio 0.1.0","store":{"backend":"postgres","slug":"memoir","database":"clio","schema":"public"},"snapshot":{"method":"offline-copy","consistency":"point-in-time","started_at":"2026-09-27T00:00:00Z","duration_ms":42},"payload":{"member":"store.db","bytes":64,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","uncompressed_bytes":64},"counts":{"complete":false},"schema_version":"1","keys":{"included":true,"member":"keys.kms.db","subjects":3,"bytes":15,"note":"key store copied; per-subject keys are wrapped and the Clio key is not included"},"label":"nightly","compression":"gzip"}"#;

pub(super) const EMPTY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The three payload members of the standard test archive, in write order.
pub(super) fn standard_members() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("store.db", {
            let mut p = b"SQLite format 3\0".to_vec();
            p.extend(vec![0xABu8; 200]);
            p
        }),
        ("keys.kms.db", b"key-store-bytes".to_vec()),
        ("dump/tables/000_items.txt", b"items\n".to_vec()),
    ]
}

/// Build the standard four-member archive (manifest + three payloads).
/// Shared with the corruption test file.
pub(super) fn write_full_archive() -> Vec<u8> {
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    for (name, bytes) in standard_members() {
        w.add_member(name, bytes.as_slice()).expect("legal member");
    }
    w.finish().expect("finish writes the end marker")
}

/// Read every member back as (name, bytes, returned digest).
pub(super) fn read_all_members(archive: &[u8]) -> Vec<(String, Vec<u8>, MemberDigest)> {
    let mut r = ArchiveReader::new(archive).expect("a written archive opens");
    let mut out = Vec::new();
    while let Some(header) = r.next_member().expect("next member") {
        let mut buf = Vec::new();
        let digest = r.read_current(&mut buf).expect("member content");
        out.push((header.name, buf, digest));
    }
    out
}

pub(super) fn sha_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    bytes_to_hex(&digest)
}

/// Copy of the container's hex encoding, to assert the digest format
/// independently of the implementation under test.
fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

/// One raw ustar member: header, payload, NUL padding. Shared with the
/// corruption test file.
pub(super) fn raw_member(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, payload.len() as u64)
        .expect("header")
        .to_vec();
    out.extend_from_slice(payload);
    let pad = (HEADER_SIZE - payload.len() % HEADER_SIZE) % HEADER_SIZE;
    out.extend(vec![0u8; pad]);
    out
}

/// Compress raw tar bytes (shared with the corruption test file).
pub(super) fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).expect("gzip write");
    e.finish().expect("gzip finish")
}

/// Decompress a written archive (shared with the corruption test file).
pub(super) fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .expect("gunzip of a written archive");
    out
}

/// A unique scratch path under the system temp directory.
pub(super) fn temp_path(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is set")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "clio-backup-container-{tag}-{}-{nanos}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}
