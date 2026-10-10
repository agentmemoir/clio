// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the backup-directory inventory: listing
//! snapshot archives and reading one archive's manifest without inflating a
//! payload.
//!
//! ## Owns
//! - Snapshot-file discovery by the snapshot file-name suffix
//! - Reading the first member (the manifest) of one archive
//!
//! ## Does not own
//! - Container encoding or reader bounds (see `container`)
//! - Manifest field definitions (see `manifest`)
//! - Backup-directory resolution (see `clio-config` `backup_path`)
//!
//! ## Boundary
//!
//! Changes to how archives are discovered or listed should normally require
//! changes here. Changes to the manifest fields should not require changes here.
//!
//! Keep this module focused on the snapshot inventory.

use crate::SNAPSHOT_EXTENSION;
use crate::container::{ArchiveReader, MAX_MANIFEST_BYTES, ReaderBounds};
use crate::manifest::BackupManifest;
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// One snapshot archive found in the backup directory.
///
/// `manifest` is `None` when the archive exists but its manifest cannot be
/// read; the entry still lists, so one corrupt file never hides the rest.
#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    pub path: PathBuf,
    pub manifest: Option<BackupManifest>,
}

/// Read one archive's manifest.
///
/// The reader is bounded to a single 64 KiB member, so only the first few
/// kilobytes of the archive are ever inflated: the payload is never read.
///
/// An archive that carries no manifest member at all - an empty archive is
/// only the end-of-archive marker - is refused with a typed
/// [`ErrorCode::ArchiveCorrupt`], never a panic. [`list_snapshots`] turns that
/// refusal into `manifest: None`, so the file still lists.
pub fn read_manifest(path: &Path) -> Result<BackupManifest, AmError> {
    let file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AmError::new(
                ErrorCode::NotFound,
                format!("no snapshot archive at {}", path.display()),
            )
        } else {
            AmError::new(
                ErrorCode::Internal,
                format!("opening {}: {e}", path.display()),
            )
        }
    })?;
    let bounds = ReaderBounds {
        max_members: 1,
        max_member_bytes: MAX_MANIFEST_BYTES,
        max_total_bytes: MAX_MANIFEST_BYTES,
    };
    let mut reader = ArchiveReader::with_bounds(file, bounds)?;
    // The container refuses a non-manifest first member, so a member here is
    // the manifest. `Ok(None)` is not a bug: the first block was the
    // end-of-archive marker, which is a valid empty archive carrying no
    // member at all, so there is no manifest to read.
    if reader.next_member()?.is_none() {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "the archive at {} carries no manifest member: the first block is the end-of-archive marker, so the archive is empty",
                path.display()
            ),
        ));
    }
    let mut json = Vec::with_capacity(4096);
    reader.read_current(&mut json)?;
    BackupManifest::from_json_bytes(&json)
}

/// List the snapshot archives in `dir`, sorted by file name.
///
/// Only file names ending in the snapshot extension are listed; other
/// files and subdirectories are ignored. An archive whose manifest cannot
/// be read still lists, with `manifest: None`.
pub fn list_snapshots(dir: &Path) -> Result<Vec<SnapshotEntry>, AmError> {
    Ok(discover(dir)?
        .into_iter()
        .map(|path| {
            let manifest = read_manifest(&path).ok();
            SnapshotEntry { path, manifest }
        })
        .collect())
}

/// One row of a backup listing: what the manifest says, plus why an archive
/// could not be read.
///
/// Every field except `unreadable` comes from the manifest member alone, so a
/// listing never inflates a payload and never opens a database. That is also
/// why there is no item count here: the manifest records the counted-set
/// completeness flag, not the numbers, and reading the numbers would mean
/// inflating the payload this listing exists to avoid.
///
/// The derived `Serialize` is the JSON field order, so a script can rely on
/// it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SnapshotRow {
    /// Snapshot id: the archive's file-name stem.
    pub id: String,
    /// The archive file.
    pub path: PathBuf,
    /// Manifest `created_at`, the ordering time. Empty when unreadable.
    pub created_at: String,
    /// Manifest `store.backend`, or `unreadable` when it could not be read.
    pub backend: String,
    /// Manifest `store.slug`, empty when unreadable.
    pub slug: String,
    /// The archive's size on disk.
    pub archive_bytes: u64,
    /// Manifest `payload.bytes`.
    pub payload_bytes: u64,
    /// Manifest `keys.included`.
    pub keys_included: bool,
    /// Manifest `label`.
    pub label: Option<String>,
    /// Why the archive could not be read, when it could not.
    pub unreadable: Option<String>,
}

/// List the snapshot archives newest first by manifest time.
///
/// The order is the manifest's `created_at`, never the file's mtime: a copied
/// or restored archive carries a meaningless mtime, and a listing that followed
/// it would put a restored snapshot at the top. Ids break a tie, so the order
/// is total. An archive whose manifest cannot be read still lists, with the
/// reason in place of its facts, and sorts after the readable ones so a broken
/// file never hides a real snapshot behind it.
///
/// `limit` keeps only the newest `limit` rows, applied after ordering.
pub fn list_snapshot_rows(dir: &Path, limit: Option<usize>) -> Result<Vec<SnapshotRow>, AmError> {
    let mut rows: Vec<SnapshotRow> = discover(dir)?
        .into_iter()
        .map(|path| row_for(&path))
        .collect();
    rows.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    Ok(rows)
}

/// The newest snapshot id in `dir`, when there is one.
pub fn latest_snapshot_id(dir: &Path) -> Result<Option<String>, AmError> {
    Ok(list_snapshot_rows(dir, Some(1))?
        .into_iter()
        .next()
        .map(|row| row.id))
}

/// One listing row, read from the manifest member alone.
fn row_for(path: &Path) -> SnapshotRow {
    let id = crate::archive_name::snapshot_id_of(path).unwrap_or_default();
    let archive_bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    match read_manifest(path) {
        Ok(manifest) => SnapshotRow {
            id,
            path: path.to_path_buf(),
            created_at: manifest.created_at,
            backend: manifest.store.backend,
            slug: manifest.store.slug,
            archive_bytes,
            payload_bytes: manifest.payload.bytes,
            keys_included: manifest.keys.included,
            label: manifest.label,
            unreadable: None,
        },
        Err(error) => SnapshotRow {
            id,
            path: path.to_path_buf(),
            created_at: String::new(),
            backend: UNREADABLE_BACKEND.to_owned(),
            slug: String::new(),
            archive_bytes,
            payload_bytes: 0,
            keys_included: false,
            label: None,
            unreadable: Some(error.message),
        },
    }
}

/// The backend a listing shows for an archive it could not read.
const UNREADABLE_BACKEND: &str = "unreadable";

/// The snapshot archive paths in `dir`, sorted by file name.
///
/// Only file names ending in the snapshot extension are returned; other files
/// and subdirectories are ignored. An entry that cannot even be read is
/// skipped, like any other file this directory does not offer as a snapshot.
fn discover(dir: &Path) -> Result<Vec<PathBuf>, AmError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AmError::new(
                ErrorCode::NotFound,
                format!("no backup directory at {}", dir.display()),
            )
        } else {
            AmError::new(
                ErrorCode::Internal,
                format!("reading {}: {e}", dir.display()),
            )
        }
    })?;

    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(SNAPSHOT_EXTENSION) {
            continue;
        }
        if !entry.path().is_file() {
            continue;
        }
        names.push(name);
    }
    names.sort();
    Ok(names.into_iter().map(|name| dir.join(name)).collect())
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "inventory_hostile_tests.rs"]
mod hostile_tests;

#[cfg(test)]
#[path = "inventory_rows_tests.rs"]
mod rows_tests;
