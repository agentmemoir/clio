// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for removing a prune's candidates from a backup
//! directory and for reporting what actually happened to each of them.
//!
//! ## Owns
//! - The per-candidate outcome: removed, already gone, skipped, or failed
//! - The containment check, so nothing outside the backup directory is removed
//! - The symlink refusal, so a linked archive is reported and never followed
//! - The byte accounting: only bytes this run really removed are reclaimed
//!
//! ## Does not own
//! - Choosing the keep set and the candidates (see `prune`)
//! - Listing the backup directory (see `inventory`)
//! - The confirmation gate, the views, and the exit codes (see `clio`)
//! - The descriptor-relative mechanics on unix (see `prune_delete_dir`)
//!
//! ## Boundary
//!
//! Changes to how a candidate is removed, or to what a removal reports, should
//! normally require changes here. Changes to which archives are targeted should
//! not require changes here.
//!
//! ## What a run guarantees
//!
//! The backup directory is resolved once and, on unix, held open as a directory
//! descriptor for the whole run. Every candidate is then examined and removed as
//! a bare file name in that one descriptor, so no ancestor of the directory can
//! be swapped under the run and no candidate path is resolved again after the
//! directory is held open. On unix the entry is also confirmed by device and
//! inode between the inspection and the unlink: a name replaced in that gap is
//! refused instead of removed, so a run never unlinks a file it did not inspect
//! and never credits a size that describes something else.
//!
//! One gap remains, and it is smaller than the one this closes: the confirming
//! stat and the unlink are not one step, so an attacker able to replace the entry
//! in the same directory between those two calls still wins. That attacker cannot
//! reach outside the held directory, because every call names a bare name inside
//! it, and cannot make the byte count describe another file, because the bytes
//! credited are the size the inspection saw and are counted only against an entry
//! the confirm matched.
//!
//! A byte is credited only when the unlink really freed it. A file another hard
//! link still holds occupies its blocks after the unlink, so that removal is
//! reported `Deleted` with nothing credited and a reason saying so. A run
//! therefore never claims a byte that is still on disk, and never credits a size
//! that describes something else.
//!
//! Other platforms have no descriptor to hold and no inode identity to compare,
//! so a removal there cannot be confirmed. A confirmed run fails such a candidate
//! and removes nothing, and each refusal is reported as a failure so the run
//! exits as a degraded outcome rather than a success. A report-only run still
//! inspects every candidate by path on those platforms, so a symlink, a directory,
//! and an unreadable name keep the report they have on unix.
//!
//! Keep this module focused on removing a selected archive.

use crate::prune::{ArchiveRef, KeptArchive, PrunePlan, UnreadableArchive};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What a prune did, or would do, with one candidate.
///
/// Every state is a fact the run observed, not a prediction: a report-only run
/// reports `Pending` for a candidate whose file is a real file right now, and a
/// confirmed run reports `Deleted` only after the unlink returned success. The
/// reason for a skip or a failure is on the outcome itself, so a state is
/// always one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// Removed from the backup directory; the run reclaimed its bytes.
    Deleted,
    /// The run was not confirmed, so the file is still there.
    Pending,
    /// Another process removed it first. Not a failure: the desired end state
    /// holds either way.
    AlreadyGone,
    /// A symbolic link. Reported and skipped, never followed.
    Symlink,
    /// The path is not a file inside the backup directory, so it was refused.
    OutsideDirectory,
    /// The removal failed; the outcome's `detail` carries the reason.
    Failed,
}

impl CandidateState {
    /// The state as the one word a report shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Pending => "pending",
            Self::AlreadyGone => "already_gone",
            Self::Symlink => "symlink",
            Self::OutsideDirectory => "outside_directory",
            Self::Failed => "failed",
        }
    }

    /// Whether this state means the candidate's removal failed.
    ///
    /// A skip is not a failure: a symlinked archive and a path outside the
    /// backup directory were never legitimate targets, and reporting either as
    /// a failure would make a correct run look broken.
    #[must_use]
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed)
    }
}

/// One candidate and what this run did with it.
///
/// `listed_bytes` is the size the inventory measured, and `reclaimed_bytes` is
/// what the run really removed. They are separate fields because a report must
/// never claim a byte it did not free: the first number says what the archive
/// occupied when the directory was listed, the second says what this run
/// unlinked, measured again immediately before the unlink.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CandidateOutcome {
    /// The snapshot id.
    pub id: String,
    /// The archive file.
    pub path: PathBuf,
    /// Manifest `created_at`, the ordering time.
    pub created_at: String,
    /// Manifest `store.backend`, so a mixed directory groups by store.
    pub backend: String,
    /// Manifest `store.slug`.
    pub slug: String,
    /// The archive's size on disk as the listing measured it.
    pub listed_bytes: u64,
    /// Bytes this run actually removed; zero unless the state is `Deleted`.
    pub reclaimed_bytes: u64,
    /// What the run did with it.
    pub state: CandidateState,
    /// The reason for a skip or a failure; `None` for the plain outcomes.
    pub detail: Option<String>,
}

/// One prune run: the keep set, every candidate's outcome, and the bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PruneReport {
    /// The `--keep` count this run applied.
    pub keep: usize,
    /// The backup directory the run was confined to.
    pub backup_dir: PathBuf,
    /// Whether the run was not confirmed, so nothing was removed.
    pub dry_run: bool,
    /// What the run kept, newest first.
    pub kept: Vec<KeptArchive>,
    /// Every candidate and what happened to it, newest first.
    pub candidates: Vec<CandidateOutcome>,
    /// Archives the listing could not read, left alone.
    pub unreadable: Vec<UnreadableArchive>,
    /// Bytes the candidates occupied when the listing measured them.
    pub reclaimable_bytes: u64,
    /// Bytes this run actually removed.
    pub reclaimed_bytes: u64,
    /// What the listing measured, minus what this run removed.
    pub remaining_bytes: u64,
}

impl PruneReport {
    /// The candidates whose removal failed.
    pub fn failed(&self) -> impl Iterator<Item = &CandidateOutcome> {
        self.candidates
            .iter()
            .filter(|outcome| outcome.state.is_failure())
    }

    /// Whether any removal failed, which is what makes a confirmed run a
    /// degraded outcome rather than a success.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.failed().next().is_some()
    }
}

/// Report what a prune would do, removing nothing.
///
/// Each candidate is still measured, so a symlinked archive and a path outside
/// the backup directory are reported as such instead of being counted as
/// deletable space. The measurement is metadata only: no archive is opened and
/// no payload is read.
#[must_use]
pub fn assess(backup_dir: &Path, plan: &PrunePlan) -> PruneReport {
    report(backup_dir, plan, false)
}

/// Remove every candidate in `plan` that this platform can confirm, and report
/// each outcome.
///
/// A candidate is removed only after the entry this run inspected is confirmed as
/// the same entry it is about to unlink. On a platform with no descriptor and no
/// inode identity that confirmation is impossible, so every candidate is refused
/// there: the run removes nothing, reports the refusal for each candidate, and
/// fails. The directory itself is never removed, even when it ends up empty, and
/// only the archive file is unlinked: no other file in the directory is touched.
/// One failing candidate does not stop the run, so a single locked file cannot
/// strand the rest; the caller sees the failure in the report.
#[must_use]
pub fn delete(backup_dir: &Path, plan: &PrunePlan) -> PruneReport {
    report(backup_dir, plan, true)
}

/// The one implementation both entry points share, with the confirmed run as a
/// parameter rather than as a second copy of the per-candidate rules.
fn report(backup_dir: &Path, plan: &PrunePlan, remove: bool) -> PruneReport {
    // The root is resolved once, before any candidate is examined, so every
    // containment check and every unlink below answers against the one real
    // directory, and on unix it is opened once here so every candidate is
    // examined and unlinked through that one descriptor. A root that cannot be
    // resolved, or cannot be opened, fails each candidate instead of falling
    // back to the unresolved path, because an unresolved path cannot prove that
    // anything is inside it.
    let root = prune_delete_dir::hold(backup_dir);
    let candidates = plan
        .candidates
        .iter()
        .map(|candidate| outcome(backup_dir, &root, candidate, remove))
        .collect::<Vec<_>>();
    // The reclaimed figure is the sum of what the run really unlinked, so it can
    // never claim a byte that is still on disk. The remaining figure is the
    // listing's own measurement minus that, so the two always agree.
    let reclaimed_bytes = candidates
        .iter()
        .map(|outcome| outcome.reclaimed_bytes)
        .fold(0u64, u64::saturating_add);
    PruneReport {
        keep: plan.keep,
        backup_dir: backup_dir.to_path_buf(),
        dry_run: !remove,
        kept: plan.kept.clone(),
        unreadable: plan.unreadable.clone(),
        reclaimable_bytes: plan.reclaimable_bytes,
        reclaimed_bytes,
        remaining_bytes: plan.listed_bytes.saturating_sub(reclaimed_bytes),
        candidates,
    }
}

/// What happened to one candidate, in the order the rules apply.
///
/// Containment comes first, so a path that is not a file inside the backup
/// directory is refused before the entry is examined at all. Being inside means
/// two things together: the caller's own spelling of the parent is exactly the
/// backup directory with a bare final component, and that parent resolves on disk
/// to the canonical root the run took once before the loop. After that every step
/// names the bare file name and never the candidate's path again.
///
/// The removal itself is `prune_delete_dir`'s, and on unix it holds the resolved
/// directory open for the whole run. One `statat` of the bare name is the
/// inspection, without following a link, and that single result decides three
/// things at once: a symlink is reported and skipped, so a link can never turn
/// "delete a file in my backup directory" into "delete a file somewhere else"; a
/// directory is refused; and the size credited to a successful removal is that
/// entry's own size. Immediately before the `unlinkat` the same bare name is
/// stat'd again and must still have the same device and inode, so a name replaced
/// between the inspection and the removal is refused with nothing removed and
/// nothing credited. Bytes are credited only when the unlink returned success and
/// the inspection saw one link to the file: a file another hard link still holds
/// keeps its blocks, so that removal is `Deleted` with nothing credited.
///
/// The one gap that remains is that the confirming stat and the `unlinkat` are
/// not one step: an attacker able to replace the entry in the same directory
/// between those two calls still wins. That attacker cannot reach outside the
/// held directory, because every call names a bare name in it, and cannot make
/// the byte count describe another file, because the bytes credited are the size
/// the inspection saw and are counted only against an entry the confirm matched.
///
/// A root that could not be resolved or opened leaves every candidate `Failed`:
/// the run then knows nothing about where the files really are, so it removes
/// nothing.
fn outcome(
    backup_dir: &Path,
    root: &Result<prune_delete_dir::BackupDir, String>,
    candidate: &ArchiveRef,
    remove: bool,
) -> CandidateOutcome {
    let mut outcome = CandidateOutcome {
        id: candidate.id.clone(),
        path: candidate.path.clone(),
        created_at: candidate.created_at.clone(),
        backend: candidate.backend.clone(),
        slug: candidate.slug.clone(),
        listed_bytes: candidate.listed_bytes,
        reclaimed_bytes: 0,
        state: CandidateState::Pending,
        detail: None,
    };
    let held = match root {
        Ok(held) => held,
        Err(reason) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(reason.clone());
            return outcome;
        }
    };
    let Some(file_name) = inside(backup_dir, held.path(), &candidate.path) else {
        outcome.state = CandidateState::OutsideDirectory;
        outcome.detail = Some(format!(
            "{} is not a file inside {}",
            candidate.path.display(),
            backup_dir.display()
        ));
        return outcome;
    };
    prune_delete_dir::remove(held, candidate, file_name, remove, outcome)
}

/// The bare file name of `path` when it is a file directly inside `backup_dir`,
/// and `None` when it is not.
///
/// The spelling is compared component-wise, so a path that reaches the file
/// through `..`, through a repeated separator, or from a sibling directory is not
/// inside and is refused. That comparison alone is a path-value test: it says
/// nothing about where the parent really is, so the parent is then canonicalized
/// and must equal the canonical root this run took. A parent that cannot be
/// canonicalized is refused for the same reason, because an unresolvable parent
/// proves nothing about containment. The check is deliberately strict: a
/// candidate the listing built as `backup_dir.join(file_name)` passes, and
/// anything else is reported rather than deleted.
///
/// This is the one place a candidate's parent is resolved, once per candidate.
/// It runs before the candidate is removed and it is what proves the caller's path
/// still names the directory the run holds open; after it, no candidate path is
/// resolved again.
fn inside<'path>(backup_dir: &Path, root: &Path, path: &'path Path) -> Option<&'path OsStr> {
    let parent = path.parent()?;
    if parent != backup_dir {
        return None;
    }
    let name = path.file_name()?;
    if name.is_empty() {
        return None;
    }
    if std::fs::canonicalize(parent).ok().as_deref() != Some(root) {
        return None;
    }
    Some(name)
}

#[cfg(test)]
#[path = "prune_delete_tests.rs"]
mod tests;

/// The descriptor-relative mechanics a removal uses: the directory held open for
/// the run, the one inspection of a bare name, the identity confirm, the unlink.
#[path = "prune_delete_dir.rs"]
mod prune_delete_dir;

/// The containment cases that need a real directory hierarchy: a backup
/// directory that is a symbolic link, a candidate reached through a linked
/// parent, and a root that cannot be resolved at all.
#[cfg(test)]
#[path = "prune_delete_containment_tests.rs"]
mod containment_tests;

/// The hard-link case: an archive another name still holds frees no block, so the
/// removal is reported and credits nothing.
#[cfg(all(test, unix))]
#[path = "prune_delete_link_tests.rs"]
mod link_tests;
