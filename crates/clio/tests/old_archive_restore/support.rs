// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared synthetic fixtures and measured
//! read-back probes the old-archive restore scenarios build on: temp
//! directories, seeded SQLite and Postgres stores, archive assembly for both
//! backends, the compliance erasure call, and the content/key observations.
//!
//! ## Owns
//! - The synthetic subjects and items, and the per-test temp directory
//! - Archive assembly: a SQLite snapshot plus key member, or a Postgres dump
//!   plus key member
//! - `readback` and `subject_key_destroyed`, the measured readability probes
//!
//! ## Does not own
//! - The restore primitives or the archive format (see `clio-store` and
//!   `clio-backup`)
//! - The scenarios and their assertions (see `sqlite`, `postgres`, `members`)
//!
//! ## Boundary
//!
//! Changes to how a synthetic fixture is seeded, archived, or read back should
//! normally require changes here. Changes to a scenario's assertions should
//! not.
//!
//! Keep this module focused on fixtures and measurement, not scenario logic.

use clio::store::{
    ComplianceStore, DekProvider, KeyPlacement, KeySwapReport, LocalDevKms, PostgresStore,
    SqliteStore, Store, SwapReport, SwapRequest, place_staged_key, swap_sqlite_store,
};
use clio::{
    Actor, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use clio_backup::manifest::ManifestAuthority;
use clio_backup::staging;
use clio_backup::{
    BackupManifest, CreateReport, CreateRequest, ExtractedArchive, KeySource, create_archive,
    extract_members, read_manifest,
};
use clio_compliance::{EraseArgs, EraseVecSink, erase_request, tombstone_salt};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::support_pg::{archive_postgres, pg_url, postgres_reachable, restore_pg};

/// The one bank every scenario seeds and reads.
pub const BANK: &str = "bank-old-archive";

/// The subject the erasure denies.
pub const DENIED_SUBJECT: &str = "subject-denied";

/// The subject the erasure never touches.
pub const CONTROL_SUBJECT: &str = "subject-control";

/// The denied subject's only item.
pub const DENIED_ITEM: &str = "item-denied";

/// The control subject's only item.
pub const CONTROL_ITEM: &str = "item-control";

/// A per-test temp directory, removed on drop.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create a fresh, unique temp directory for one scenario.
    ///
    /// The directory is created exclusively, so a stale directory another run
    /// left at the same name is never reused; a collision retries with a fresh
    /// name. The directory is removed when this guard drops.
    pub fn new(tag: &str) -> Self {
        for _ in 0..64 {
            let path = std::env::temp_dir().join(unique(tag));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create the scratch directory {}: {error}", path.display()),
            }
        }
        panic!("could not create a unique scratch directory for {tag}");
    }

    /// Join a relative path under the scratch directory.
    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A unique string per call, so two tests never share a path or a name.
pub fn unique(tag: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("old-archive-{tag}-{}-{sequence}", std::process::id())
}

/// The owner-only journal path for one scenario's erasures: a sibling entry of
/// the per-test scratch root under the system temp root, outside every path a
/// restore replaces (never the database file or its sidecars, the key store,
/// or the backup/staging directories).
///
/// A caller that lets the write reach this path owns removing it; the negative
/// scenario refuses before any journal write, so it leaves nothing behind.
pub fn journal_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(unique(tag))
}

/// One synthetic item: dual content, sealed under the subject's DEK on write.
fn item(id: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: BANK.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "synthetic old-archive subject"})),
        gist: Some(gist.to_owned()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Write one synthetic item into an open store.
pub fn write_item<S: Store>(store: &S, id: &str, subject: &str, gist: &str) {
    store
        .create_memory_item(&item(id, subject, gist), "harness")
        .expect("write the synthetic item");
}

/// Open a SQLite store backed by the file key store at `kms_path`.
pub fn open_sqlite(db: &Path, kms_path: &Path) -> SqliteStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    SqliteStore::open(db, kms).expect("open the sqlite store")
}

/// Open a Postgres store backed by the file key store at `kms_path`.
pub fn open_pg(url: &str, kms_path: &Path) -> PostgresStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    PostgresStore::open(url, kms).expect("open the postgres store")
}

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
/// under, so the guarded restore can classify its claim.
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
    let scratch = Scratch::new("erase-journal");
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
        timestamp: "2026-09-17T11:00:00Z",
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

/// Print one compact per-route observation line.
pub fn observe(
    backend: &str,
    route: &str,
    archive: &Archive,
    content: &Readback,
    key_destroyed: bool,
) {
    println!(
        "OBS backend={backend} route={route} archive={} content={content:?} \
         key_destroyed={key_destroyed} keys_included={} key_subjects={}",
        archive.id, archive.manifest.keys.included, archive.manifest.keys.subjects
    );
}
