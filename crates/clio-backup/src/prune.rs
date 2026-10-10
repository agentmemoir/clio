// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for choosing which snapshot archives a prune
//! keeps and which it may delete, from a backup listing alone.
//!
//! ## Owns
//! - The keep set, the prune candidates, and the unreadable archives, decided
//!   from the listing and a `--keep` count
//! - The rule that the newest archive and the only archive of a store are never
//!   candidates
//! - The rule that a symbolic link is not a snapshot: it takes no keep slot,
//!   satisfies no store's floor, and contributes no bytes
//! - The refusal for a `--keep` of zero
//!
//! ## Does not own
//! - Listing a backup directory (see `inventory`)
//! - Telling a link from a real file (see `prune_kind`)
//! - Removing an archive or measuring what a removal freed (see `prune_delete`)
//! - The confirmation gate, the views, and the exit codes (see `clio`)
//!
//! ## Boundary
//!
//! Changes to which archives a prune targets should normally require changes
//! here. Changes to how a candidate is removed should not require changes here.
//!
//! Keep this module focused on selection, which touches no filesystem.

use crate::inventory::SnapshotRow;
use crate::prune_kind::LinkedPaths;
use std::collections::HashMap;
use std::path::PathBuf;

/// Why an archive the plan keeps is kept.
///
/// Both reasons keep an archive alive under every accepted `--keep`: the first
/// is the keep window itself, the second is the per-store floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepReason {
    /// One of the newest `keep` archives by manifest time.
    KeepWindow,
    /// The only readable archive of its store, so deleting it would leave that
    /// store with no snapshot at all.
    LastOfStore,
}

/// One archive the report names, with everything the report may say about it.
///
/// The fields are the identity, the time, the store it belongs to, and its size
/// on disk. No other manifest field is carried: a report describes archives, it
/// does not quote their manifests, so nothing here can print a label the
/// operator wrote or a fact about their store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArchiveRef {
    /// The snapshot id: the archive's file-name stem.
    pub id: String,
    /// The archive file.
    pub path: PathBuf,
    /// Manifest `created_at`, the ordering time.
    pub created_at: String,
    /// Manifest `store.backend`.
    pub backend: String,
    /// Manifest `store.slug`.
    pub slug: String,
    /// The archive's size on disk, as the listing measured it.
    pub listed_bytes: u64,
}

/// One archive the plan keeps, with the rule that kept it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeptArchive {
    /// The archive itself.
    #[serde(flatten)]
    pub archive: ArchiveRef,
    /// Why this archive stays.
    pub reason: KeepReason,
}

/// An archive the listing could not read, and why.
///
/// An unreadable archive is never a candidate: it may be a half-written archive
/// from a killed process, a snapshot from a future format version, or a file
/// the operator put there. Deciding that is the operator's call, so the plan
/// reports it and leaves it alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnreadableArchive {
    /// The archive's file-name stem; the id a listing would show.
    pub id: String,
    /// The archive file.
    pub path: PathBuf,
    /// The archive's size on disk, as the listing measured it.
    pub listed_bytes: u64,
    /// Why the manifest could not be read.
    pub reason: String,
}

/// What a prune keeps, what it may delete, and what it leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePlan {
    /// The `--keep` count this plan was built for.
    pub keep: usize,
    /// What the plan keeps, newest first by manifest time.
    pub kept: Vec<KeptArchive>,
    /// What the plan may delete, newest first by manifest time.
    ///
    /// A symbolic link is in this list so the report names it, with zero bytes,
    /// because the delete half skips it rather than unlinking it.
    pub candidates: Vec<ArchiveRef>,
    /// Archives whose manifest could not be read, so never candidates.
    pub unreadable: Vec<UnreadableArchive>,
    /// Total bytes the candidates occupied when the listing measured them.
    ///
    /// A linked candidate contributes nothing: a run never removes a link, so
    /// its target's bytes are space this command cannot free.
    pub reclaimable_bytes: u64,
    /// Total bytes of every archive the listing reported.
    ///
    /// A link is not an archive and is not measured through, so it contributes
    /// nothing here either; its target is counted on its own row.
    pub listed_bytes: u64,
}

/// Why a `--keep` count was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneRefusal {
    /// `--keep 0` would leave the newest snapshot deletable.
    KeepZero,
}

/// The refusal message for a `PruneRefusal`, naming the reason.
#[must_use]
pub fn refusal_message(refusal: PruneRefusal) -> String {
    match refusal {
        PruneRefusal::KeepZero => {
            "`--keep 0` was refused: the newest snapshot is never a candidate".to_owned()
        }
    }
}

/// Decide the keep set and the candidates from a backup listing.
///
/// `rows` is the listing exactly as `list_snapshot_rows` returns it: newest
/// first by manifest time, with an unreadable archive last. The plan reads that
/// order and never re-sorts it, never re-scans the directory, and never reads a
/// manifest, so `prune` and `list` cannot disagree about which archive is
/// newest. The file's mtime plays no part: a copied or restored archive carries
/// an mtime that means nothing.
///
/// `links` is the caller's answer to which listed entries are symbolic links
/// ([`LinkedPaths::among`]). It is a parameter rather than a `stat` so that
/// selection stays a pure function of the listing: the filesystem is read once,
/// by the caller, and the answer arrives as data. A link is not a snapshot, so
/// it takes no keep slot, satisfies no store's floor, and contributes no bytes
/// to either total; it stays in the plan as a candidate so the report names it,
/// and the delete half reports it as the link it is.
///
/// Two rules hold for every accepted `keep`. The keep window is the newest
/// `keep` readable archives, so the newest archive is never a candidate. Beyond
/// the window, the only readable archive of a store is still kept, because a
/// backup directory may legitimately hold several stores and deleting a store's
/// last snapshot would leave that store with nothing.
///
/// # Errors
/// `keep` is 0 ([`PruneRefusal::KeepZero`]), which would make the newest
/// snapshot deletable.
pub fn select(
    rows: &[SnapshotRow],
    links: &LinkedPaths,
    keep: usize,
) -> Result<PrunePlan, PruneRefusal> {
    if keep == 0 {
        return Err(PruneRefusal::KeepZero);
    }
    // A link is left out of the readable set and of the per-store count, so it
    // cannot push a real archive out of the keep window and cannot stand in as
    // a store's only snapshot. An unreadable archive is reported separately and
    // is never a candidate, as before.
    let (readable, unreadable): (Vec<&SnapshotRow>, Vec<&SnapshotRow>) = rows
        .iter()
        .filter(|row| !links.contains(&row.path))
        .partition(|row| row.unreadable.is_none());

    // How many readable archives each store contributes, so the only archive of
    // a store can be recognized without a second pass over the order.
    let mut per_store: HashMap<(&str, &str), usize> = HashMap::new();
    for row in &readable {
        *per_store
            .entry((row.backend.as_str(), row.slug.as_str()))
            .or_default() += 1;
    }

    let mut kept = Vec::new();
    let mut candidates = Vec::new();
    let mut in_readable = 0usize;
    for row in rows {
        if links.contains(&row.path) {
            // The link is reported as a candidate so the operator sees it, and it
            // carries no bytes: the archive it points at is measured on its own
            // row, and counting the target twice would claim space this command
            // can never free.
            candidates.push(archive_ref(row, 0));
            continue;
        }
        if row.unreadable.is_some() {
            continue;
        }
        let in_window = in_readable < keep;
        in_readable += 1;
        let last_of_store = per_store[&(row.backend.as_str(), row.slug.as_str())] == 1;
        let archive = archive_ref(row, row.archive_bytes);
        if in_window || last_of_store {
            kept.push(KeptArchive {
                archive,
                reason: if in_window {
                    KeepReason::KeepWindow
                } else {
                    KeepReason::LastOfStore
                },
            });
        } else {
            candidates.push(archive);
        }
    }

    let unreadable = unreadable
        .into_iter()
        .map(|row| UnreadableArchive {
            id: row.id.clone(),
            path: row.path.clone(),
            listed_bytes: row.archive_bytes,
            reason: row.unreadable.clone().unwrap_or_default(),
        })
        .collect();
    let reclaimable_bytes = candidates.iter().map(|archive| archive.listed_bytes).sum();
    let listed_bytes = rows
        .iter()
        .filter(|row| !links.contains(&row.path))
        .map(|row| row.archive_bytes)
        .fold(0u64, u64::saturating_add);
    Ok(PrunePlan {
        keep,
        kept,
        candidates,
        unreadable,
        reclaimable_bytes,
        listed_bytes,
    })
}

/// The report's view of one listing row, measured at `listed_bytes`.
///
/// The size is a parameter because a link is not measured through: its row's
/// size is the target's, which belongs to the target's own row.
fn archive_ref(row: &SnapshotRow, listed_bytes: u64) -> ArchiveRef {
    ArchiveRef {
        id: row.id.clone(),
        path: row.path.clone(),
        created_at: row.created_at.clone(),
        backend: row.backend.clone(),
        slug: row.slug.clone(),
        listed_bytes,
    }
}

#[cfg(test)]
#[path = "prune_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "prune_selection_tests.rs"]
mod selection_tests;
