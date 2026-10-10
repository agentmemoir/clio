// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that the bytes an archive carries
//! can be taken back out and used: a real archive is created and verified,
//! then its payload member and its key member are extracted into a fresh
//! directory and compared with the manifest and with the key store that went
//! in.
//!
//! ## Owns
//! - The extraction helper and the restore-readiness cases that use it
//!
//! ## Does not own
//! - Recalling an item. `clio backup restore` does not exist yet, and this
//!   crate depends on neither `clio-store` nor a database, so no recall can
//!   run from here. What this file proves is the part that can be proved
//!   inside the crate: both members come back out byte-exact, the key member
//!   is a SQLite key-store database, and a damaged key member is caught.
//! - Create, verify, and container cases (see their own test modules)
//!
//! ## Boundary
//!
//! Changes to what a restore needs from an archive should normally require
//! changes here. Changes to how a member is written or checked should not.

use crate::KEY_MEMBER;
use crate::container::{ArchiveReader, MAX_MANIFEST_BYTES, MemberDigest, ReaderBounds};
use crate::create::create_archive;
use crate::fixtures::{Scratch, dir_entries, key_source, request, staged_payload};
use crate::inventory::read_manifest;
use crate::manifest::BackupManifest;
use crate::staging::{self, StagedFile};
use crate::tar_header::{HEADER_SIZE, build_header};
use crate::verify::verify_archive;
use clio_types::AmError;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// SQLite file magic, the first 16 bytes of every key-store database.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// One extracted member: where it landed and what the reader computed while
/// streaming it out.
#[derive(Debug)]
struct Extracted {
    name: String,
    path: PathBuf,
    digest: MemberDigest,
}

/// The extracted members and the staged files that own them. Dropping the
/// value removes the extracted files, so a failing test leaves nothing behind.
struct Restore {
    members: Vec<Extracted>,
    _staged: Vec<StagedFile>,
}

impl Restore {
    fn member(&self, name: &str) -> &Extracted {
        self.members
            .iter()
            .find(|member| member.name == name)
            .unwrap_or_else(|| panic!("{name} was not extracted: {:?}", self.members))
    }
}

/// A real archive with a payload member and a key member, plus the manifest
/// inside it and the key-store file that went in.
struct Snapshot {
    archive: PathBuf,
    manifest: BackupManifest,
    key_store: PathBuf,
}

/// Create an archive through `create_archive`, so the members and the manifest
/// are the ones the product writes.
fn snapshot_with_keys(scratch: &Scratch) -> Snapshot {
    let payload = staged_payload(scratch.path(), 61);
    let mut req = request(scratch.path(), payload);
    let keys = key_source(scratch.path(), 3);
    req.keys = Some(keys.clone());
    let report = create_archive(&req).expect("create an archive with a key member");
    // The caller owns the staged payload, exactly as the CLI does.
    drop(req);
    let manifest = read_manifest(&report.path).expect("read the manifest back");
    assert!(manifest.keys.included, "the create carried a key member");
    Snapshot {
        archive: report.path,
        manifest,
        key_store: keys.path,
    }
}

/// Extract the payload member and the key member into `dir`, and return each
/// file with the digest the reader computed while streaming it out.
///
/// The caps are explicit, as they are everywhere else in the crate, and taken
/// from the manifest rather than invented: a member whose header claims more
/// than the manifest recorded is refused before a byte of it is inflated. The
/// manifest records no size for the key member, so the per-member cap leaves
/// it the manifest cap's worth of headroom.
fn extract_members(archive: &Path, manifest: &BackupManifest, dir: &Path) -> Restore {
    let per_member = manifest.payload.bytes + MAX_MANIFEST_BYTES;
    let bounds = ReaderBounds {
        max_members: 4,
        max_member_bytes: per_member,
        max_total_bytes: 2 * per_member,
    };
    let file = std::fs::File::open(archive).expect("open the archive");
    let mut reader = ArchiveReader::with_bounds(file, bounds).expect("open the reader");
    let mut members: Vec<Extracted> = Vec::new();
    let mut staged: Vec<StagedFile> = Vec::new();
    while let Some(header) = reader.next_member().expect("next member") {
        if header.name != manifest.payload.member && header.name != KEY_MEMBER {
            reader.skip_current().expect("skip a member nobody needs");
            continue;
        }
        let (path, mut out) =
            staging::begin_staged(dir, "restore-extract").expect("owner-only file");
        let digest = reader
            .read_current(&mut out)
            .expect("stream the member out of the archive");
        out.sync_all().expect("flush the extracted member");
        staged.push(staging::adopt_staged(
            path.clone(),
            digest.size,
            digest.sha256.clone(),
        ));
        members.push(Extracted {
            name: header.name,
            path,
            digest,
        });
    }
    Restore {
        members,
        _staged: staged,
    }
}

/// Lowercase hex, written out here so the digest is not checked with the same
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

/// One raw ustar member: header, payload, NUL padding.
fn raw_member(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, payload.len() as u64)
        .expect("header")
        .to_vec();
    out.extend_from_slice(payload);
    out.extend(crate::fixtures::pad(payload));
    out
}

/// Compress raw tar bytes.
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// Rebuild the archive with the key member's first 16 bytes replaced, leaving
/// the manifest and every other member exactly as they were.
fn rewrite_key_head(archive: &Path, replacement: &[u8]) {
    let file = std::fs::File::open(archive).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    let mut raw: Vec<u8> = Vec::new();
    while let Some(header) = reader.next_member().expect("next member") {
        let mut body = Vec::new();
        reader.read_current(&mut body).expect("member body");
        if header.name == KEY_MEMBER {
            body[..SQLITE_MAGIC.len()].copy_from_slice(replacement);
        }
        raw.extend(raw_member(&header.name, &body));
    }
    raw.extend([0u8; 2 * HEADER_SIZE]);
    std::fs::write(archive, gzip(&raw)).expect("write the rebuilt archive");
}

/// A stub integrity check standing in for the store half, so this file needs
/// no database.
#[allow(clippy::unnecessary_wraps)]
fn integrity_ok(_staged: &Path) -> Result<(), AmError> {
    Ok(())
}

/// A verified archive hands back its payload member and its key member: both
/// are on disk in a fresh directory, both match the manifest, and the key
/// member is byte-identical to the key store that went in.
#[test]
fn a_verified_archive_hands_back_its_payload_and_key_members_intact() {
    let scratch = Scratch::new("restore-extract");
    let snapshot = snapshot_with_keys(&scratch);

    // The injected integrity check stands in for the store half and records
    // the bytes it was handed, which is how the payload's landing on disk is
    // proved rather than assumed.
    let mut calls = 0;
    let mut seen: Vec<u8> = Vec::new();
    let report = {
        let mut integrity = |staged: &Path| -> Result<(), AmError> {
            calls += 1;
            seen = std::fs::read(staged)
                .map_err(|error| staging::io_error("reading the staged payload", &error))?;
            Ok(())
        };
        verify_archive(&snapshot.archive, &mut integrity).expect("verify the archive")
    };

    assert!(report.ok, "failed checks: {:?}", report.failed());
    assert_eq!(calls, 1, "the store half is called once");
    assert_eq!(
        seen.len() as u64,
        snapshot.manifest.payload.bytes,
        "the staged payload is the manifest's size"
    );
    assert_eq!(
        sha_hex(&seen),
        snapshot.manifest.payload.sha256,
        "the staged payload is the manifest's digest"
    );
    for member in [&snapshot.manifest.payload.member, KEY_MEMBER] {
        let check = report
            .members
            .iter()
            .find(|check| check.member == *member)
            .unwrap_or_else(|| panic!("{member} is not in the report: {:?}", report.members));
        assert!(check.ok, "{}: {}", check.member, check.detail);
    }

    let fresh = scratch.join("restored");
    assert!(
        staging::ensure_backup_dir(&fresh).expect("create the fresh data directory"),
        "the fresh directory is new"
    );
    let restore = extract_members(&snapshot.archive, &snapshot.manifest, &fresh);
    assert_eq!(
        restore.members.len(),
        2,
        "the payload and the key member come back out"
    );

    let payload = restore.member(&snapshot.manifest.payload.member);
    assert_eq!(payload.digest.size, snapshot.manifest.payload.bytes);
    assert_eq!(payload.digest.sha256, snapshot.manifest.payload.sha256);
    let payload_bytes = std::fs::read(&payload.path).expect("read the extracted payload back");
    assert_eq!(payload_bytes.len() as u64, snapshot.manifest.payload.bytes);
    assert_eq!(sha_hex(&payload_bytes), snapshot.manifest.payload.sha256);

    let keys = restore.member(KEY_MEMBER);
    let source = std::fs::read(&snapshot.key_store).expect("read the key store that went in");
    let key_bytes = std::fs::read(&keys.path).expect("read the extracted key member back");
    assert_eq!(
        key_bytes, source,
        "the key member is a verbatim copy, not a rewritten one"
    );
    assert_eq!(keys.digest.size, source.len() as u64);
    assert_eq!(keys.digest.sha256, sha_hex(&source));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for member in &restore.members {
            let mode = std::fs::metadata(&member.path)
                .expect("member metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{} is owner-only", member.name);
        }
        let dir_mode = std::fs::metadata(&fresh)
            .expect("directory metadata")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "the data directory is owner-only");
    }

    let mut expected: Vec<String> = restore
        .members
        .iter()
        .map(|member| {
            member
                .path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    expected.sort();
    assert_eq!(
        dir_entries(&fresh),
        expected,
        "nothing but the two extracted members is left in the directory"
    );
}

/// The extracted key member is a SQLite key-store database, not a truncated
/// or empty stand-in: the strongest check available inside this crate, and
/// what makes the key-member path proven rather than merely present.
#[test]
fn the_extracted_key_member_is_a_sqlite_key_store_database() {
    let scratch = Scratch::new("restore-keys-sqlite");
    let snapshot = snapshot_with_keys(&scratch);
    let fresh = scratch.join("restored");
    assert!(staging::ensure_backup_dir(&fresh).expect("create the fresh data directory"));
    let restore = extract_members(&snapshot.archive, &snapshot.manifest, &fresh);

    let keys = restore.member(KEY_MEMBER);
    let key_bytes = std::fs::read(&keys.path).expect("read the extracted key member back");
    assert!(
        key_bytes.len() > SQLITE_MAGIC.len(),
        "the key member is longer than its head: {} bytes",
        key_bytes.len()
    );
    assert_eq!(
        &key_bytes[..SQLITE_MAGIC.len()],
        SQLITE_MAGIC,
        "the key member is a SQLite key-store database"
    );
    assert_eq!(
        keys.digest.size,
        key_bytes.len() as u64,
        "it is not truncated"
    );

    let payload = restore.member(&snapshot.manifest.payload.member);
    let payload_bytes = std::fs::read(&payload.path).expect("read the extracted payload back");
    assert_eq!(
        &payload_bytes[..SQLITE_MAGIC.len()],
        SQLITE_MAGIC,
        "the payload member is a store database too, so the pair is usable"
    );
    assert_eq!(restore.members.len(), 2, "no other member came back out");
}

/// A key member whose head is damaged is caught: the verify pass names the key
/// member as failed, and the extracted copy is visibly not a database. A
/// single flipped byte is the smallest damage that changes the answer.
#[test]
fn a_damaged_key_member_head_is_caught_by_the_check_and_the_extraction() {
    let scratch = Scratch::new("restore-keys-damaged");
    let snapshot = snapshot_with_keys(&scratch);
    let before = std::fs::read(&snapshot.key_store).expect("read the key store");
    rewrite_key_head(&snapshot.archive, b"NotSQLite format");

    let report = verify_archive(&snapshot.archive, &mut integrity_ok).expect("the container reads");

    assert!(!report.ok, "a key member that is not a key store must fail");
    assert!(
        report.failed().contains(&KEY_MEMBER),
        "the key member is named: {:?}",
        report.failed()
    );
    let check = report
        .members
        .iter()
        .find(|check| check.member == KEY_MEMBER)
        .expect("the key member is in the report");
    assert!(
        check.detail.contains("is not a SQLite key-store database"),
        "the report says why: {}",
        check.detail
    );

    let fresh = scratch.join("restored");
    assert!(staging::ensure_backup_dir(&fresh).expect("create the fresh data directory"));
    let restore = extract_members(&snapshot.archive, &snapshot.manifest, &fresh);
    let extracted = std::fs::read(&restore.member(KEY_MEMBER).path).expect("read the key member");
    assert_ne!(&extracted[..SQLITE_MAGIC.len()], SQLITE_MAGIC);
    assert_ne!(extracted, before, "the damage is the one that was written");
}
