// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared measurement surface of the
//! cross-path compliance proof: assembling a SQLite archive, restoring it with
//! or without its key member, running the real compliance erasure, and reading
//! an item and its subject key back to classify what survived.
//!
//! ## Owns
//! - `Archive`, `archive_sqlite`, and `archive_sqlite_with_authority`
//! - `restore_sqlite`, `restore_key_only`, `extract_into`, `key_source`, and
//!   `publish`
//! - `erase_subject`, `Readback`, `readback`, and `subject_key_destroyed`
//!
//! ## Does not own
//! - The synthetic subjects, scratch directory, and seeded stores (see
//!   `fixtures`)
//! - The Postgres archive and restore (see `support_pg`)
//! - The scenarios and their assertions (see the `scenario_*` modules)
//!
//! ## Boundary
//!
//! Changes to how an archive is assembled or read back should normally require
//! changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on archive assembly and measurement, not on
//! scenario logic.

use crate::fixtures::{BANK, scratch};
use clio::store::{
    ComplianceStore, KeyPlacement, KeySwapReport, Store, SwapReport, SwapRequest, place_staged_key,
    swap_sqlite_store,
};
use clio::{Actor, Bank, ErrorCode, OpContext};
use clio_backup::manifest::ManifestAuthority;
use clio_backup::staging;
use clio_backup::{
    BackupManifest, CreateReport, CreateRequest, ExtractedArchive, KeySource, create_archive,
    extract_members, read_manifest,
};
use clio_compliance::{EraseArgs, EraseVecSink, erase_request, tombstone_salt};
use std::path::{Path, PathBuf};

/// One assembled archive and the manifest it carries.
pub struct Archive {
    /// The snapshot id: the archive file-name stem.
    pub id: String,
    /// Absolute path of the published archive.
    pub path: PathBuf,
    /// The manifest read back from the archive.
    pub manifest: BackupManifest,
}

/// Assemble a SQLite archive from the live store at `db`, carrying `kms_path`.
pub fn archive_sqlite(backup_dir: &Path, db: &Path, kms_path: &Path, slug: &str) -> Archive {
    archive_sqlite_with_authority(backup_dir, db, kms_path, slug, None)
}

/// Assemble a SQLite archive that records the erasure `authority` it was made
/// under, so a guarded restore can classify its claim.
pub fn archive_sqlite_with_authority(
    backup_dir: &Path,
    db: &Path,
    kms_path: &Path,
    slug: &str,
    authority: Option<ManifestAuthority>,
) -> Archive {
    staging::ensure_backup_dir(backup_dir).expect("create the backup directory");
    let staged = backup_dir.join("payload.stage");
    let facts = clio::store::snapshot_sqlite_into(db, &staged).expect("snapshot the sqlite store");
    let payload = staging::adopt_staged(staged, facts.bytes, facts.sha256.clone());
    let request = CreateRequest {
        backup_dir: backup_dir.to_path_buf(),
        slug: slug.to_owned(),
        backend: "sqlite".to_owned(),
        database: db
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        store_schema: None,
        schema_version: facts.schema_version.clone(),
        payload,
        payload_member: clio::store::PAYLOAD_MEMBER.to_owned(),
        additional_members: Vec::new(),
        method: clio::store::METHOD.to_owned(),
        consistency: clio::store::CONSISTENCY.to_owned(),
        started_at: facts.started_at.clone(),
        duration_ms: facts.duration_ms,
        counts_complete: true,
        keys: key_source(kms_path),
        label: None,
        note: None,
        authority,
        tool_version: clio::version().to_owned(),
    };
    publish(create_archive(&request).expect("create the sqlite archive"))
}

/// The key member an archive carries, or `None` when the target keeps keys in
/// process memory.
pub(crate) fn key_source(kms_path: &Path) -> Option<KeySource> {
    clio::store::key_member_source(Some(kms_path))
        .expect("resolve the key member")
        .map(|source| KeySource {
            path: source.path,
            subjects: source.subjects,
        })
}

/// Read the published archive's id and manifest back.
pub(crate) fn publish(report: CreateReport) -> Archive {
    let manifest = read_manifest(&report.path).expect("read the archive manifest");
    Archive {
        id: report.id,
        path: report.path,
        manifest,
    }
}

/// Replace the live SQLite payload (and, when asked, the key store) with the
/// archive's members. Returns the swap report.
pub fn restore_sqlite(
    archive: &Archive,
    target_db: &Path,
    target_kms: &Path,
    with_key: bool,
) -> SwapReport {
    let dir = target_db.parent().expect("the target has a parent");
    std::fs::create_dir_all(dir).expect("create the target directory");
    let extracted = extract_into(archive, dir);
    let key = with_key.then(|| KeyPlacement {
        staged: extracted
            .key
            .as_ref()
            .expect("the archive carries a key member")
            .file
            .path()
            .to_path_buf(),
        destination: target_kms.to_path_buf(),
    });
    swap_sqlite_store(&SwapRequest {
        staged_payload: extracted.payload.file.path().to_path_buf(),
        target: target_db.to_path_buf(),
        key,
    })
    .expect("swap the sqlite store")
}

/// Place the archive's key member over the live key store, with no payload swap.
pub fn restore_key_only(archive: &Archive, target_kms: &Path) -> KeySwapReport {
    let dir = target_kms.parent().expect("the target has a parent");
    std::fs::create_dir_all(dir).expect("create the target directory");
    let extracted = extract_into(archive, dir);
    let member = extracted
        .key
        .as_ref()
        .expect("the archive carries a key member");
    place_staged_key(member.file.path(), target_kms)
}

/// Extract an archive's declared members into `dir`.
pub(crate) fn extract_into(archive: &Archive, dir: &Path) -> ExtractedArchive {
    extract_members(&archive.path, &archive.manifest, dir).expect("extract the archive members")
}

/// Destroy one subject's DEK through the real compliance erasure call.
pub fn erase_subject(store: &dyn ComplianceStore, subject: &str, request_id: &str) {
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let salt = tombstone_salt();
    // The journal lives in its own per-call scratch directory: it stays outside
    // every path a restore replaces, and goes away with the guard on drop.
    let scratch = scratch("erase-journal");
    let journal = scratch.join("erasure-journal.json");
    let outcome = erase_request(&EraseArgs {
        store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: subject,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-01-01T00:00:00Z",
        sink: &sink,
    })
    .expect("run the erasure");
    assert_eq!(outcome.status, "COMPLETED", "the erasure did not complete");
}

/// What reading one item back observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readback {
    /// The item's content decrypted.
    Readable,
    /// The item's row was present but its DEK is destroyed.
    DestroyedKey,
    /// The item's row was present but no DEK slot exists.
    KeyMissing,
    /// No row for the item.
    Absent,
    /// Any other failure, named by its error code.
    Failed(String),
}

/// Read one item back and classify what happened.
pub fn readback<S: Store + ?Sized>(store: &S, item_id: &str) -> Readback {
    match store.get_memory_item(BANK, item_id) {
        Ok(Some(_)) => Readback::Readable,
        Ok(None) => Readback::Absent,
        Err(error) if error.code == ErrorCode::ErasedSubject => Readback::DestroyedKey,
        Err(error) if error.code == ErrorCode::Forbidden => Readback::KeyMissing,
        Err(error) => Readback::Failed(error.code.as_str().to_owned()),
    }
}

/// Whether the subject's DEK is destroyed in the store's key material.
pub fn subject_key_destroyed(store: &dyn ComplianceStore, subject: &str) -> bool {
    store
        .subject_dek_destroyed(subject)
        .expect("probe the subject's key")
}
