// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bounded-manifest hostile cases: the
//! archives a hostile or damaged file can be, read through `read_manifest`,
//! `list_snapshot_rows`, and `verify_archive` — member order abuse, a manifest
//! over the cap, and hostile operator text.
//!
//! ## Owns
//! - The hostile raw-tar fixtures and the assertions on what the reader, the
//!   listing, and the verify pass actually do with them
//! - The shared hostile-case helpers (`manifest_bytes`, `raw_member`,
//!   `hostile_archive`, `refuse`, `verify_refuses`, `integrity_ok`) used by
//!   the duplicate-member cases too
//!
//! ## Does not own
//! - The container's header and stream refusals (see
//!   `container_corrupt_tests`) and the bare empty-archive listing case (see
//!   `inventory_hostile_tests`)
//! - The reader's repeated-name rule itself (see `container_reader`); the
//!   duplicate-member cases live in `hostile_duplicate_tests`
//! - Any other fix. A hostile case that exposes a defect the reader already
//!   refuses is asserted here; one it does not refuse is reported, not patched
//!   into the reader from a test.
//!
//! ## Boundary
//!
//! Changes to what these hostile archives must provoke should normally require
//! changes here. Changes to the container's own refusals should not.

use crate::KEY_MEMBER;
use crate::MANIFEST_MEMBER;
use crate::container::{MAX_MANIFEST_BYTES, to_hex};
use crate::fixtures::{Scratch, dir_entries, pad};
use crate::inventory::{list_snapshot_rows, list_snapshots, read_manifest};
use crate::manifest::{COMPRESSION, KIND};
use crate::tar_header::{HEADER_SIZE, build_header};
use crate::verify::verify_archive;
use clio_types::AmError;
use clio_types::ErrorCode;
use clio_types::ErrorCode::ArchiveCorrupt;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Payload bytes for a hostile archive. They start with the SQLite magic, so
/// only the container and the manifest can object to them.
pub(crate) const PAYLOAD: &[u8] = b"SQLite format 3\0payload-bytes";

/// A key store, and a second, different one.
pub(crate) const KEYS: &[u8] = b"SQLite format 3\0key-store-bytes";

/// A manifest as bytes: the fields a hostile case varies are arguments and the
/// rest are fixed. Every case here reads this back through
/// `BackupManifest::from_json_bytes`, so it is validated by the same code a
/// real archive's manifest is.
pub(crate) fn manifest_bytes(payload: &[u8], keys: bool, label: Option<&str>) -> Vec<u8> {
    let value = serde_json::json!({
        "kind": KIND,
        "format_version": 1,
        "created_at": "2026-09-27T00:00:00Z",
        "tool_version": "clio 0.1.0",
        "store": {"backend": "sqlite", "slug": "clio", "database": "clio", "schema": null},
        "snapshot": {"method": "vacuum-into", "consistency": "point-in-time",
                     "started_at": "2026-09-27T00:00:00Z", "duration_ms": 42},
        "payload": {"member": "store.db", "bytes": payload.len(),
                    "sha256": to_hex(&Sha256::digest(payload)),
                    "uncompressed_bytes": payload.len()},
        "counts": {"complete": true},
        "schema_version": null,
        "keys": {"included": keys, "member": keys.then_some(KEY_MEMBER), "subjects": 2,
                 "note": "key store copied; per-subject keys are wrapped and the Clio key is not included"},
        "label": label,
        "note": null,
        "compression": COMPRESSION
    });
    serde_json::to_vec(&value).expect("valid JSON")
}

/// One raw ustar member: header, payload, NUL padding.
pub(crate) fn raw_member(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, payload.len() as u64)
        .expect("header")
        .to_vec();
    out.extend_from_slice(payload);
    out.extend(pad(payload));
    out
}

/// Compress raw tar bytes.
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// A manifest member, a payload member, and the end-of-archive marker.
fn manifest_and_payload(manifest: &[u8]) -> Vec<u8> {
    let mut raw = raw_member(MANIFEST_MEMBER, manifest);
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    raw
}

/// Write raw tar bytes as a snapshot file in `dir` and return its path.
pub(crate) fn hostile_archive(dir: &Path, tag: &str, raw: &[u8]) -> PathBuf {
    let path = dir.join(format!("{tag}.cliobak.tar.gz"));
    std::fs::write(&path, gzip(raw)).expect("write the hostile archive");
    path
}

/// A manifest member, a payload member, and the marker, as a snapshot file.
fn manifest_archive(scratch: &Scratch, tag: &str, manifest: &[u8]) -> PathBuf {
    let raw = manifest_and_payload(manifest);
    hostile_archive(scratch.path(), tag, &raw)
}

/// The message of a refusal, after checking the code it has to carry.
pub(crate) fn refuse(result: Result<impl Debug, AmError>, code: ErrorCode) -> String {
    let error = result.expect_err("the archive must be refused");
    assert_eq!(error.code, code, "{}", error.message);
    error.message
}

/// The verify pass refuses the same archive, with the same words.
pub(crate) fn verify_refuses(path: &Path, code: ErrorCode) -> String {
    refuse(verify_archive(path, &mut integrity_ok), code)
}

/// A stub integrity check standing in for the store half, so these cases need
/// no database.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn integrity_ok(_staged: &Path) -> Result<(), AmError> {
    Ok(())
}

/// The scratch directory holds the one hostile archive and nothing else: no
/// read of a hostile archive wrote a file.
fn nothing_written_beside(scratch: &Scratch, tag: &str) {
    let only = vec![format!("{tag}.cliobak.tar.gz")];
    assert_eq!(dir_entries(scratch.path()), only, "nothing was written");
}

/// Member-order abuse. An archive whose payload member comes before the
/// manifest member, and an archive that carries payload members and no
/// manifest member at all, are both refused by name through every entry point:
/// the message states the rule and names the member it found. The bare empty
/// archive — the marker blocks and nothing else — has its own case in
/// `inventory_hostile_tests`; these two look complete except for the manifest.
#[test]
fn member_order_abuse_is_refused_by_name_through_every_entry_point() {
    let scratch = Scratch::new("hostile-order");
    let manifest = manifest_bytes(PAYLOAD, false, None);
    let mut payload_first = raw_member("store.db", PAYLOAD);
    payload_first.extend(raw_member(MANIFEST_MEMBER, &manifest));
    payload_first.extend(vec![0u8; 2 * HEADER_SIZE]);
    let path = hostile_archive(scratch.path(), "payload-first", &payload_first);

    let message = refuse(read_manifest(&path), ArchiveCorrupt);
    assert!(
        message.contains("first archive member must be"),
        "{message}"
    );
    assert!(message.contains(MANIFEST_MEMBER), "the rule: {message}");
    assert!(message.contains("\"store.db\""), "what it found: {message}");
    assert_eq!(verify_refuses(&path, ArchiveCorrupt), message);
    // The listing keeps the file and states why, instead of dropping it.
    let rows = list_snapshot_rows(scratch.path(), None).expect("the listing is total");
    assert_eq!(rows.len(), 1, "the hostile file still lists");
    assert_eq!(rows[0].unreadable.as_deref(), Some(message.as_str()));
    assert_eq!(rows[0].backend, "unreadable");
    assert_eq!(rows[0].label, None);
    nothing_written_beside(&scratch, "payload-first");

    let mut no_manifest = raw_member("store.db", PAYLOAD);
    no_manifest.extend(raw_member(KEY_MEMBER, KEYS));
    no_manifest.extend(vec![0u8; 2 * HEADER_SIZE]);
    let path = hostile_archive(scratch.path(), "no-manifest", &no_manifest);
    let message = refuse(read_manifest(&path), ArchiveCorrupt);
    assert!(
        message.contains("first archive member must be"),
        "{message}"
    );
    assert_eq!(verify_refuses(&path, ArchiveCorrupt), message);
    let entries = list_snapshots(scratch.path()).expect("the listing is total");
    assert_eq!(entries.len(), 2, "both hostile files still list");
    assert!(entries[1].manifest.is_none(), "it lists with no manifest");
}

/// A manifest member over the cap is refused on its declared size, before a
/// single byte of it is inflated, and the refusal names the member, the size it
/// claimed, and the cap.
#[test]
fn a_manifest_member_over_the_cap_is_refused_before_any_of_it_is_read() {
    let scratch = Scratch::new("hostile-bigmanifest");
    // The header claims more than the cap while the stream holds far less, so
    // a reader that inflated first would fail on a short member instead.
    let head = b"{\"kind\":\"clio-store-backup\"";
    let over = MAX_MANIFEST_BYTES + 1;
    let mut raw = build_header(MANIFEST_MEMBER, over)
        .expect("header")
        .to_vec();
    raw.extend_from_slice(head);
    raw.extend(pad(head));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    let path = hostile_archive(scratch.path(), "big-manifest", &raw);

    // The cap is the reader's bound on this archive, so the code is the one that
    // means "this file is not readable by this build", not the machine's quota.
    let message = refuse(read_manifest(&path), ArchiveCorrupt);
    assert!(message.contains("per-member cap"), "the rule: {message}");
    let cap = MAX_MANIFEST_BYTES.to_string();
    assert!(message.contains(&cap), "the limit in bytes: {message}");
    assert!(message.contains(MANIFEST_MEMBER), "the member: {message}");
    assert_eq!(verify_refuses(&path, ArchiveCorrupt), message);
    let rows = list_snapshot_rows(scratch.path(), None).expect("the listing is total");
    assert_eq!(rows.len(), 1, "the file still lists");
    assert_eq!(rows[0].unreadable.as_deref(), Some(message.as_str()));
}

/// A manifest just under the cap that carries a very long label reads. The
/// writer bounds operator text at 200 characters; the read path applies no
/// rule of its own, so the whole label comes back.
#[test]
fn a_hostile_label_just_under_the_cap_comes_back_whole() {
    let scratch = Scratch::new("hostile-long-label");
    let label = "L".repeat(60_000);
    let manifest = manifest_bytes(PAYLOAD, false, Some(&label));
    let under_cap = manifest.len() as u64 <= MAX_MANIFEST_BYTES;
    assert!(under_cap, "under the cap: {} bytes", manifest.len());
    let path = manifest_archive(&scratch, "long-label", &manifest);

    let read = read_manifest(&path).expect("a manifest under the cap reads");
    assert_eq!(read.label.as_deref(), Some(label.as_str()), "not shortened");
    let rows = list_snapshot_rows(scratch.path(), None).expect("the listing is total");
    assert_eq!(rows[0].label.as_deref(), Some(label.as_str()));
    assert!(rows[0].unreadable.is_none());
    let report = verify_archive(&path, &mut integrity_ok).expect("verify");
    assert!(report.ok, "the verify pass objects to nothing: {report:?}");
}

/// A label holding a newline and a NUL is returned verbatim by the read path,
/// and the listing escapes it, so a hostile label cannot present a viewer with
/// a second record.
#[test]
fn a_control_character_in_a_label_is_verbatim_but_escaped_in_the_listing() {
    let scratch = Scratch::new("hostile-control-label");
    let label = "nightly\nrun\0injected";
    let manifest = manifest_bytes(PAYLOAD, false, Some(label));
    let path = manifest_archive(&scratch, "control-label", &manifest);

    let read = read_manifest(&path).expect("the manifest reads");
    assert_eq!(read.label.as_deref(), Some(label), "read back verbatim");
    let rows = list_snapshot_rows(scratch.path(), None).expect("the listing is total");
    assert_eq!(rows[0].label.as_deref(), Some(label));
    let rendered = serde_json::to_string(&rows).expect("render the row");
    let raw_control = rendered.contains('\n') || rendered.contains('\0');
    assert!(!raw_control, "no raw control character: {rendered}");
    let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON back");
    assert_eq!(parsed[0]["label"], serde_json::json!(label), "round trip");
}

/// A manifest member that is not JSON — here bytes that are not even UTF-8 —
/// is refused with the parser's own reason. The manifest is present and
/// unreadable, which is what this case is about.
#[test]
fn a_manifest_member_that_is_not_json_is_refused_with_the_parse_reason() {
    let scratch = Scratch::new("hostile-garbage-manifest");
    let garbage = vec![0xFFu8; 512];
    let path = manifest_archive(&scratch, "garbage-manifest", &garbage);

    let message = refuse(read_manifest(&path), ArchiveCorrupt);
    assert!(message.contains("invalid manifest JSON"), "{message}");
    let longer = message.len() > "invalid manifest JSON".len();
    assert!(longer, "the parser's own reason is kept: {message}");
    assert_eq!(verify_refuses(&path, ArchiveCorrupt), message);
    let entries = list_snapshots(scratch.path()).expect("the listing is total");
    assert!(entries[0].manifest.is_none(), "it lists with no manifest");
}
