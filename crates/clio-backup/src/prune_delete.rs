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
//!
//! ## Boundary
//!
//! Changes to how a candidate is removed, or to what a removal reports, should
//! normally require changes here. Changes to which archives are targeted should
//! not require changes here.
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

/// Remove every candidate in `plan` and report each outcome.
///
/// The directory itself is never removed, even when it ends up empty, and only
/// the archive file is unlinked: no other file in the directory is touched. One
/// failing candidate does not stop the run, so a single locked file cannot
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
    // directory. A root that cannot be resolved fails each candidate instead of
    // falling back to the unresolved path, because an unresolved path cannot
    // prove that anything is inside it.
    let root = std::fs::canonicalize(backup_dir);
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
/// directory is refused before the filesystem is touched at all. Being inside
/// means two things together: the caller's own spelling of the parent is exactly
/// the backup directory with a bare final component, and that parent resolves on
/// disk to the canonical root the run took once before the loop. Then the entry
/// is examined with `symlink_metadata`, which does not follow a link: a
/// symlinked archive is reported and skipped, so a link can never turn "delete a
/// file in my backup directory" into "delete a file somewhere else". Only a real
/// file is unlinked, and the unlink names the resolved root plus the bare file
/// name, so no unresolved or `..`-bearing path reaches the removal.
///
/// What this does not prove is that the entry examined and the entry unlinked are
/// the same entry: the name can be swapped in the gap between the
/// `symlink_metadata` and the `remove_file`, and a regular file swapped in there
/// is the file this run removes. A swap can therefore make the reclaimed figure
/// describe something other than what went. Removing that gap needs the
/// directory itself held open for the run and the entry confirmed by device and
/// inode across the two calls.
///
/// A root that could not be resolved leaves every candidate `Failed`: the run
/// then knows nothing about where the files really are, so it removes nothing.
fn outcome(
    backup_dir: &Path,
    root: &Result<PathBuf, std::io::Error>,
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
    let root = match root {
        Ok(root) => root.as_path(),
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!(
                "the backup directory {} could not be resolved, so nothing was removed: {error}",
                backup_dir.display()
            ));
            return outcome;
        }
    };
    let Some(file_name) = inside(backup_dir, root, &candidate.path) else {
        outcome.state = CandidateState::OutsideDirectory;
        outcome.detail = Some(format!(
            "{} is not a file inside {}",
            candidate.path.display(),
            backup_dir.display()
        ));
        return outcome;
    };
    let metadata = match std::fs::symlink_metadata(&candidate.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            outcome.state = CandidateState::AlreadyGone;
            outcome.detail = Some("another process removed it first".to_owned());
            return outcome;
        }
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!("cannot read the archive: {error}"));
            return outcome;
        }
    };
    if metadata.is_symlink() {
        outcome.state = CandidateState::Symlink;
        outcome.detail = Some(
            "a symbolic link is never followed; its target's bytes are not counted".to_owned(),
        );
        return outcome;
    }
    if metadata.is_dir() {
        outcome.state = CandidateState::Failed;
        outcome.detail = Some("the path is a directory, not a snapshot archive".to_owned());
        return outcome;
    }
    if !remove {
        return outcome;
    }
    // The size is measured here, immediately before the unlink, so the reclaimed
    // figure is the bytes this run freed rather than a number the listing wrote
    // down earlier. It is counted only after the unlink reports success. The
    // unlink names the resolved root and the bare file name, so the removal
    // cannot travel through a symlinked or `..`-bearing parent.
    let bytes = metadata.len();
    let removal_path = root.join(file_name);
    match std::fs::remove_file(&removal_path) {
        Ok(()) => {
            outcome.state = CandidateState::Deleted;
            outcome.reclaimed_bytes = bytes;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            outcome.state = CandidateState::AlreadyGone;
            outcome.detail = Some("another process removed it first".to_owned());
        }
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!("removing {}: {error}", candidate.path.display()));
        }
    }
    outcome
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

/// The containment cases that need a real directory hierarchy: a backup
/// directory that is a symbolic link, a candidate reached through a linked
/// parent, and a root that cannot be resolved at all.
#[cfg(test)]
#[path = "prune_delete_containment_tests.rs"]
mod containment_tests;
