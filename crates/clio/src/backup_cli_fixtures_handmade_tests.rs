// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hand-built archive fixture: an archive
//! assembled through the container writer with manifest fields a test chooses
//! freely and a fixed identity, plus the root-user guard the unwritable-
//! directory cases share.
//!
//! ## Owns
//! - `hand_built_archive`, whose manifest fields, id, and stamps a test sets
//! - `running_as_root`, the guard for the mode-bit cases
//!
//! ## Does not own
//! - The create-path fixtures (see `backup_cli_fixtures`)
//! - Any assertion: a fixture is data, never a test of its own
//!
//! ## Boundary
//!
//! Changes to what the hand-built archive carries belong here. Changes to the
//! assertions that use it do not.

use crate::backup_cli_fixtures::{Scratch, ensure_journal, key_store_path};
use clio_store::{DekProvider, LocalDevKms};
use std::path::{Path, PathBuf};

/// An archive built by hand through the container writer, so a test chooses
/// the manifest fields a refusal turns on and fixes the identity a golden
/// test pins: the id is the archive's file-name stem, and the stamps and
/// backend are the fixed values a restore test needs.
///
/// The payload is the store file as it sits on disk (the store is closed), and
/// the manifest records the bytes actually staged, so a hand-built archive
/// with an untouched manifest is restorable. `duplicate_key` adds a second
/// `keys.kms.db` member built from a different key store; the archive is then
/// assembled from raw members because the writer correctly refuses the
/// repeat, and only the reader's repeated-name rule is under test.
///
/// Returns the id and the archive path.
pub(crate) fn hand_built_archive(
    scratch: &Scratch,
    store: &Path,
    id: &str,
    duplicate_key: bool,
    adjust: impl FnOnce(&mut clio_backup::BackupManifest),
) -> (String, PathBuf) {
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let staged = clio_backup::staging::stage_copy(store, &backup_dir, "handmade")
        .expect("stage the store as a payload");
    let bytes = std::fs::read(staged.path()).expect("read the staged payload");
    // The key member's own size, which is what the manifest now records, so it
    // is measured here rather than taken from the payload's byte count.
    let key_bytes = std::fs::metadata(key_store_path(store))
        .expect("the store's key store")
        .len();
    let mut manifest = clio_backup::BackupManifest {
        kind: clio_backup::KIND.to_owned(),
        format_version: clio_backup::FORMAT_VERSION,
        created_at: "2026-09-27T00:00:00Z".to_owned(),
        tool_version: "clio 0.1.0".to_owned(),
        store: clio_backup::StoreInfo {
            backend: "sqlite".to_owned(),
            slug: "clio".to_owned(),
            database: store
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            schema: None,
        },
        snapshot: clio_backup::SnapshotInfo {
            method: "vacuum-into".to_owned(),
            consistency: "point-in-time".to_owned(),
            started_at: "2026-09-27T00:00:00Z".to_owned(),
            duration_ms: 1,
        },
        payload: clio_backup::PayloadInfo {
            member: "store.db".to_owned(),
            bytes: bytes.len() as u64,
            sha256: staged.sha256().to_owned(),
            uncompressed_bytes: bytes.len() as u64,
        },
        counts: clio_backup::CountsInfo { complete: true },
        schema_version: Some("1".to_owned()),
        keys: clio_backup::KeysInfo {
            included: true,
            member: Some(clio_backup::KEY_MEMBER.to_owned()),
            subjects: 1,
            bytes: Some(key_bytes),
            note: "the local key store is carried verbatim".to_owned(),
        },
        label: None,
        note: None,
        compression: clio_backup::COMPRESSION.to_owned(),
        authority: Some(ensure_journal(scratch)),
    };
    adjust(&mut manifest);
    drop(staged);
    let key = std::fs::read(key_store_path(store)).expect("read the key store");
    let path = backup_dir.join(clio_backup::archive_file_name(id));
    if duplicate_key {
        let second = scratch.join("second.kms.json");
        let kms = LocalDevKms::open_persistent(&second).expect("open the second key store");
        kms.ensure_dek("subject-0000").expect("write a slot");
        drop(kms);
        let other = std::fs::read(&second).expect("read the second key store");
        // The writer refuses a repeated member name, so the duplicate archive
        // is assembled from raw members through the container's test support
        // instead of through the writer.
        let archive = clio_backup::container::archive_from_raw_members_for_tests(&[
            (clio_backup::MANIFEST_MEMBER, &manifest.to_json_bytes()[..]),
            ("store.db", &bytes[..]),
            (clio_backup::KEY_MEMBER, &key[..]),
            (clio_backup::KEY_MEMBER, &other[..]),
        ]);
        std::fs::write(&path, archive).expect("write the archive");
    } else {
        let mut writer = clio_backup::ArchiveWriter::new(Vec::new());
        writer
            .write_manifest(&manifest.to_json_bytes())
            .expect("write the manifest");
        writer
            .add_member("store.db", &bytes[..])
            .expect("add the payload");
        writer
            .add_member(clio_backup::KEY_MEMBER, &key[..])
            .expect("add the key member");
        std::fs::write(&path, writer.finish().expect("finish the archive"))
            .expect("write the archive");
    }
    (id.to_owned(), path)
}

/// Root ignores mode bits, so the unwritable-directory cases only run for a
/// normal user. `/proc/self/status` exists on Linux; where it is absent the
/// case runs anyway.
#[cfg(unix)]
pub(crate) fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .map(|uids| uids.split_whitespace().next() == Some("0"))
        })
        .unwrap_or(false)
}
