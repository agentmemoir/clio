// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing `extract`: real archives hand their
//! members back out byte-exact, and archives that disagree with their manifest
//! are refused with the member named.
//!
//! ## Owns
//! - The extract cases and the raw-rewrite helpers they build archives with
//!
//! ## Does not own
//! - The extraction under test and the sibling member suites
//!
//! ## Boundary
//!
//! Changes to what extraction must satisfy belong here; extraction internals
//! do not.

use crate::KEY_MEMBER;
use crate::container::ArchiveReader;
use crate::create::create_archive;
use crate::extract::staging_tests::staged_names;
use crate::extract::{ExtractedArchive, extract_members};
use crate::fixtures::{
    SQLITE_MAGIC, Scratch, dir_entries, key_source, payload_bytes, request, staged_payload,
};
use crate::inventory::read_manifest;
use crate::manifest::BackupManifest;
use crate::staging;
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::{AmError, ErrorCode};
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// Payload seed shared by the fixtures, so the expected bytes are known.
const PAYLOAD_SEED: u8 = 61;

/// A real archive built by `create_archive`, with the manifest inside it.
struct Snapshot {
    archive: PathBuf,
    manifest: BackupManifest,
    key_store: Option<PathBuf>,
}

/// Create an archive for `scratch`, with a key member when `with_keys`.
fn snapshot(scratch: &Scratch, with_keys: bool) -> Snapshot {
    let payload = staged_payload(scratch.path(), PAYLOAD_SEED);
    let mut req = request(scratch.path(), payload);
    let key_store = if with_keys {
        let keys = key_source(scratch.path(), 3);
        req.keys = Some(keys.clone());
        Some(keys.path)
    } else {
        None
    };
    let report = create_archive(&req).expect("create the archive");
    let manifest = read_manifest(&report.path).expect("read the manifest back");
    Snapshot {
        archive: report.path,
        manifest,
        key_store,
    }
}

/// A fresh owner-only directory to extract into.
fn fresh_dir(scratch: &Scratch) -> PathBuf {
    let dir = scratch.join("restored");
    assert!(
        staging::ensure_backup_dir(&dir).expect("create the staging directory"),
        "the staging directory is new"
    );
    dir
}

/// Extract `archive` into a fresh directory.
fn extract(scratch: &Scratch, archive: &Path) -> (ExtractedArchive, PathBuf) {
    let dir = fresh_dir(scratch);
    let manifest = read_manifest(archive).expect("the archive's manifest");
    let extracted = extract_members(archive, &manifest, &dir).expect("extract the archive");
    (extracted, dir)
}

/// The refusal extracting `archive` into a fresh directory produces.
fn refusal(scratch: &Scratch, archive: &Path) -> (AmError, PathBuf) {
    let dir = fresh_dir(scratch);
    let manifest = read_manifest(archive).expect("the archive's manifest");
    let Err(error) = extract_members(archive, &manifest, &dir) else {
        panic!("the archive is refused");
    };
    (error, dir)
}

/// Every member of `archive` as a `(name, body)` pair, in archive order.
fn members_of(archive: &Path) -> Vec<(String, Vec<u8>)> {
    let file = std::fs::File::open(archive).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("open the reader");
    let mut members = Vec::new();
    while let Some(header) = reader.next_member().expect("next member") {
        let mut body = Vec::new();
        reader.read_current(&mut body).expect("read a member body");
        members.push((header.name, body));
    }
    members
}

/// One raw ustar member: header, payload, NUL padding.
fn raw_member(name: &str, body: &[u8]) -> Vec<u8> {
    raw_member_declaring(name, body.len() as u64, body)
}

/// One raw ustar member whose header declares `declared` bytes while the body
/// holds whatever bytes the caller supplies; the padding follows the body.
fn raw_member_declaring(name: &str, declared: u64, body: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, declared).expect("header").to_vec();
    out.extend_from_slice(body);
    out.extend(crate::fixtures::pad(body));
    out
}

/// Compress raw tar bytes.
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// Write `members` back as a gzip archive with the end-of-archive marker.
fn write_members(archive: &Path, members: &[(String, Vec<u8>)]) {
    let mut raw: Vec<u8> = Vec::new();
    for (name, body) in members {
        raw.extend(raw_member(name, body));
    }
    raw.extend([0u8; 2 * HEADER_SIZE]);
    std::fs::write(archive, gzip(&raw)).expect("write the rebuilt archive");
}

/// Lowercase hex, written out here so a digest is not checked with the same
/// code the reader used to compute it.
fn sha_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
        .map(char::from)
        .collect()
}

/// A real archive hands back its payload and its key member byte-exact, both
/// owner-only, and stages only those two files.
#[test]
fn a_real_archive_hands_back_its_payload_and_its_key_member() {
    let scratch = Scratch::new("extract-both");
    let snapshot = snapshot(&scratch, true);
    let (extracted, dir) = extract(&scratch, &snapshot.archive);

    let id = crate::archive_name::snapshot_id_of(&snapshot.archive).expect("an id");
    assert_eq!(extracted.id, id);
    assert_eq!(extracted.path, snapshot.archive);
    assert!(
        format!("{extracted:?}").contains(&extracted.id),
        "debug names the id"
    );
    assert_eq!(extracted.payload.member, snapshot.manifest.payload.member);
    assert_eq!(
        extracted.payload.file.bytes(),
        snapshot.manifest.payload.bytes
    );
    assert_eq!(
        extracted.payload.file.sha256(),
        snapshot.manifest.payload.sha256
    );

    let payload_on_disk = std::fs::read(extracted.payload.file.path()).expect("read the payload");
    assert_eq!(payload_on_disk, payload_bytes(PAYLOAD_SEED), "byte-exact");
    assert_eq!(sha_hex(&payload_on_disk), snapshot.manifest.payload.sha256);

    let key = extracted
        .key
        .as_ref()
        .expect("the key member comes back out");
    assert_eq!(key.member, KEY_MEMBER);
    let key_store = snapshot.key_store.as_ref().expect("a key store went in");
    let expected_key = std::fs::read(key_store).expect("read the key store");
    let key_on_disk = std::fs::read(key.file.path()).expect("read the key member");
    assert_eq!(key_on_disk, expected_key, "a verbatim copy");
    assert_eq!(key.file.bytes(), expected_key.len() as u64);

    #[cfg(unix)]
    for file in [extracted.payload.file.path(), key.file.path()] {
        let mode = std::fs::metadata(file)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{} is owner-only", file.display());
    }

    staged_names(&dir, &[extracted.payload.file.path(), key.file.path()]);
}

/// A keyless archive hands back only its payload.
#[test]
fn a_keyless_archive_hands_back_only_its_payload() {
    let scratch = Scratch::new("extract-keyless");
    let snapshot = snapshot(&scratch, false);
    assert!(!snapshot.manifest.keys.included);
    let (extracted, dir) = extract(&scratch, &snapshot.archive);

    assert!(extracted.key.is_none(), "no key member comes back out");
    let payload_on_disk = std::fs::read(extracted.payload.file.path()).expect("read the payload");
    assert_eq!(payload_on_disk, payload_bytes(PAYLOAD_SEED));
    assert_eq!(
        extracted.payload.file.sha256(),
        snapshot.manifest.payload.sha256
    );
    staged_names(&dir, &[extracted.payload.file.path()]);
}

/// A payload whose header size disagrees with the manifest is refused before a
/// byte of it is inflated, naming the member and both numbers.
#[test]
fn a_payload_header_size_the_manifest_does_not_record_is_refused() {
    let scratch = Scratch::new("extract-size-mismatch");
    let snapshot = snapshot(&scratch, false);
    let declared = snapshot.manifest.payload.bytes + 1;
    let mut raw: Vec<u8> = Vec::new();
    for (name, body) in &members_of(&snapshot.archive) {
        if *name == snapshot.manifest.payload.member {
            raw.extend(raw_member_declaring(name, declared, body));
        } else {
            raw.extend(raw_member(name, body));
        }
    }
    raw.extend([0u8; 2 * HEADER_SIZE]);
    std::fs::write(&snapshot.archive, gzip(&raw)).expect("write the rebuilt archive");

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(&snapshot.manifest.payload.member)
            && error.message.contains(&declared.to_string())
            && error
                .message
                .contains(&snapshot.manifest.payload.bytes.to_string()),
        "the member and both sizes are named: {}",
        error.message
    );
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}

/// A payload with one flipped byte is refused, naming the member, and the copy is removed.
#[test]
fn a_tampered_payload_byte_is_refused_and_leaves_no_staged_file() {
    let scratch = Scratch::new("extract-tampered");
    let snapshot = snapshot(&scratch, false);
    let mut members = members_of(&snapshot.archive);
    for (name, body) in &mut members {
        if *name == snapshot.manifest.payload.member {
            body[0] ^= 0xFF;
        }
    }
    write_members(&snapshot.archive, &members);

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(&snapshot.manifest.payload.member),
        "the member is named: {}",
        error.message
    );
    assert!(error.message.contains("digest"), "the digest is named");
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}

/// A truncated payload is refused, naming the member, and the copy is removed.
#[test]
fn a_truncated_archive_is_refused() {
    let scratch = Scratch::new("extract-truncated");
    let snapshot = snapshot(&scratch, false);
    let mut raw: Vec<u8> = Vec::new();
    for (name, body) in &members_of(&snapshot.archive) {
        if *name == snapshot.manifest.payload.member {
            let half = &body[..body.len() / 2];
            raw.extend(raw_member_declaring(name, body.len() as u64, half));
        } else {
            raw.extend(raw_member(name, body));
        }
    }
    std::fs::write(&snapshot.archive, gzip(&raw)).expect("write the rebuilt archive");

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(&snapshot.manifest.payload.member),
        "the member is named: {}",
        error.message
    );
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}

/// A key member the manifest does not declare is refused, and the payload is removed.
#[test]
fn a_key_member_the_manifest_does_not_declare_is_refused() {
    let scratch = Scratch::new("extract-undeclared-key");
    let snapshot = snapshot(&scratch, false);
    let mut members = members_of(&snapshot.archive);
    let mut key_body = SQLITE_MAGIC.to_vec();
    key_body.extend(std::iter::repeat_n(0x5Au8, 64));
    members.push((KEY_MEMBER.to_owned(), key_body));
    write_members(&snapshot.archive, &members);

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(KEY_MEMBER),
        "the key member is named: {}",
        error.message
    );
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}

/// A key member whose head is not SQLite magic is refused and named.
#[test]
fn a_key_member_whose_head_is_not_sqlite_is_refused() {
    let scratch = Scratch::new("extract-bad-key");
    let snapshot = snapshot(&scratch, true);
    let mut members = members_of(&snapshot.archive);
    for (name, body) in &mut members {
        if name == KEY_MEMBER {
            body[..SQLITE_MAGIC.len()].copy_from_slice(b"NotSQLite format");
        }
    }
    write_members(&snapshot.archive, &members);

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains(KEY_MEMBER),
        "the key member is named: {}",
        error.message
    );
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}

/// An archive that does not carry a member the manifest declares is refused,
/// naming the missing member.
#[test]
fn an_archive_that_does_not_carry_a_declared_member_is_refused() {
    for with_keys in [false, true] {
        let scratch = Scratch::new("extract-missing-member");
        let snapshot = snapshot(&scratch, with_keys);
        let mut dropped = KEY_MEMBER.to_owned();
        if !with_keys {
            dropped.clone_from(&snapshot.manifest.payload.member);
        }
        let members: Vec<(String, Vec<u8>)> = members_of(&snapshot.archive)
            .into_iter()
            .filter(|(name, _)| *name != dropped)
            .collect();
        write_members(&snapshot.archive, &members);

        let (error, dir) = refusal(&scratch, &snapshot.archive);

        assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
        assert!(
            error.message.contains(&dropped),
            "missing member: {}",
            error.message
        );
        assert!(
            dir_entries(&dir).is_empty(),
            "staged files were left: {:?}",
            dir_entries(&dir)
        );
    }
}

/// An unlisted member whose bytes end early is refused and named.
#[test]
fn an_unlisted_member_that_ends_early_is_refused() {
    let scratch = Scratch::new("extract-unlisted");
    let snapshot = snapshot(&scratch, false);
    let mut raw: Vec<u8> = Vec::new();
    for (name, body) in &members_of(&snapshot.archive) {
        raw.extend(raw_member(name, body));
    }
    raw.extend(raw_member_declaring("extra.bin", 1024, &[0xAB; 8]));
    std::fs::write(&snapshot.archive, gzip(&raw)).expect("write the rebuilt archive");

    let (error, dir) = refusal(&scratch, &snapshot.archive);

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains("extra.bin"),
        "the member is named: {}",
        error.message
    );
    assert!(
        dir_entries(&dir).is_empty(),
        "staged files were left: {:?}",
        dir_entries(&dir)
    );
}
