// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the file-system mechanics a prune uses on one
//! candidate: resolving and holding the backup directory, examining one bare
//! name in it, confirming the same entry is still there, and removing it.
//!
//! ## Owns
//! - The directory the whole run holds open, and the one open that gets it
//! - The one inspection of a bare name: its type and the size a removal credits
//! - The device-and-inode identity confirm between the inspection and the unlink
//! - The unlink of a bare name in that same held directory
//! - The path-based removal other platforms keep instead
//!
//! ## Does not own
//! - Which bare name is a candidate (`prune_delete::inside` decides that)
//! - What a state means in a report, or the byte totals (`prune_delete` maps
//!   the states this module returns into a report)
//!
//! ## Boundary
//!
//! Changes to how a candidate is examined or removed at the file-system level
//! should normally require changes here. Changes to what a removal reports
//! should not.
//!
//! ## Why the descriptor
//!
//! A path resolved twice can name two different files: between the inspection
//! and the removal the entry can be replaced by a different regular file, and a
//! run that then removed it would delete something it never inspected while
//! crediting the length it did inspect. Holding the resolved directory open once
//! and naming only a bare file name in it removes the second resolution, so no
//! ancestor of the backup directory can change under the run; confirming device
//! and inode across the two calls then refuses a name replaced in the gap
//! instead of removing it.
//!
//! Unix only: the calls are POSIX `openat`/`fstatat`/`unlinkat`. Other platforms
//! have no descriptor to hold and no inode identity to compare, so this module
//! keeps the path-based `symlink_metadata`/`remove_file` form there (see
//! `remove`).

use super::{ArchiveRef, CandidateOutcome, CandidateState};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The backup directory a run answers against: where it really is, and on unix
/// the descriptor it is held open under.
pub(super) struct BackupDir {
    /// The canonical path the run resolved once.
    path: PathBuf,
    /// The directory held open for the whole run.
    #[cfg(unix)]
    dir: std::os::fd::OwnedFd,
}

impl BackupDir {
    /// Where the directory really is.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// Resolve `backup_dir` once and, on unix, open it once as a directory.
///
/// The error is the reason no candidate of this run can be removed, already
/// worded for the report, so the caller fails every candidate with the same
/// explanation and never falls back to a path-based removal: a removal decided
/// against an unresolvable or unopenable directory is exactly the removal this
/// module exists to prevent.
pub(super) fn hold(backup_dir: &Path) -> Result<BackupDir, String> {
    let path = std::fs::canonicalize(backup_dir).map_err(|error| {
        format!(
            "the backup directory {} could not be resolved, so nothing was removed: {error}",
            backup_dir.display()
        )
    })?;
    // `NOFOLLOW` on the final component, so a directory component swapped in
    // after the resolution above is refused rather than followed.
    #[cfg(unix)]
    let dir = rustix::fs::open(
        &path,
        rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| {
        format!(
            "the backup directory {} could not be opened, so nothing was removed: {error}",
            backup_dir.display(),
            error = io_error(error)
        )
    })?;
    Ok(BackupDir {
        path,
        #[cfg(unix)]
        dir,
    })
}

/// Remove one candidate whose bare `file_name` is inside the held directory.
///
/// Every call below names `file_name`, a bare name in the held directory, and
/// never the candidate's path again. The single `statat` is the inspection: the
/// symlink refusal, the directory refusal, and the size a successful removal
/// credits all come from it. The name is then stat'd a second time and the two
/// results must share device and inode, so a name replaced after it was inspected
/// is refused with nothing removed and nothing credited. Bytes are credited only
/// after `unlinkat` returned success, from the inspection's own size.
#[cfg(unix)]
pub(super) fn remove(
    held: &BackupDir,
    candidate: &ArchiveRef,
    file_name: &OsStr,
    remove: bool,
    mut outcome: CandidateOutcome,
) -> CandidateOutcome {
    use rustix::fs::{AtFlags, FileType, RawMode, statat, unlinkat};
    use rustix::io::Errno;

    let inspected = match statat(&held.dir, file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return gone(outcome),
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!("cannot read the archive: {}", io_error(error)));
            return outcome;
        }
    };
    match FileType::from_raw_mode(inspected.st_mode as RawMode) {
        FileType::Symlink => {
            outcome.state = CandidateState::Symlink;
            outcome.detail = Some(
                "a symbolic link is never followed; its target's bytes are not counted".to_owned(),
            );
            return outcome;
        }
        FileType::Directory => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some("the path is a directory, not a snapshot archive".to_owned());
            return outcome;
        }
        _ => {}
    }
    if !remove {
        return outcome;
    }
    // The entry is confirmed before it is removed: a name replaced after the
    // inspection is refused rather than unlinked, so this run never removes a
    // file it did not inspect and never credits a size that describes another
    // file. An unreadable second look proves nothing, so it refuses too.
    match statat(&held.dir, file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(confirmed) if same_entry(&inspected, &confirmed) => {}
        Ok(_) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!(
                "{} changed after it was inspected, so it was not removed",
                candidate.path.display()
            ));
            return outcome;
        }
        Err(Errno::NOENT) => return gone(outcome),
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!(
                "cannot confirm the archive before removing it: {}",
                io_error(error)
            ));
            return outcome;
        }
    }
    match unlinkat(&held.dir, file_name, AtFlags::empty()) {
        Ok(()) => {
            outcome.state = CandidateState::Deleted;
            outcome.reclaimed_bytes = u64::try_from(inspected.st_size).unwrap_or(0);
        }
        Err(Errno::NOENT) => return gone(outcome),
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!(
                "removing {}: {}",
                candidate.path.display(),
                io_error(error)
            ));
        }
    }
    outcome
}

/// Remove one candidate by path, on a platform with no directory descriptor and
/// no inode identity to compare.
///
/// This is the pre-descriptor sequence, unchanged in effect: one
/// `symlink_metadata`, which does not follow a link, then one `remove_file` on
/// the bare name in the resolved root. The descriptor and the identity confirm
/// are unix-only, so the check-then-use gap is not narrowed here by any check.
#[cfg(not(unix))]
pub(super) fn remove(
    held: &BackupDir,
    candidate: &ArchiveRef,
    file_name: &OsStr,
    remove: bool,
    mut outcome: CandidateOutcome,
) -> CandidateOutcome {
    let metadata = match std::fs::symlink_metadata(&candidate.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return gone(outcome),
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
    let bytes = metadata.len();
    match std::fs::remove_file(held.path.join(file_name)) {
        Ok(()) => {
            outcome.state = CandidateState::Deleted;
            outcome.reclaimed_bytes = bytes;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return gone(outcome),
        Err(error) => {
            outcome.state = CandidateState::Failed;
            outcome.detail = Some(format!("removing {}: {error}", candidate.path.display()));
        }
    }
    outcome
}

/// The outcome of a candidate another process removed first. Not a failure: the
/// desired end state holds either way.
fn gone(mut outcome: CandidateOutcome) -> CandidateOutcome {
    outcome.state = CandidateState::AlreadyGone;
    outcome.detail = Some("another process removed it first".to_owned());
    outcome
}

/// Whether the second stat names the same entry as the first.
///
/// Device and inode together are an entry's identity on a unix file system: a
/// name replaced by a different file, or by a link, has a different inode, while
/// a hard link to the same file has the same one. The fields are compared
/// directly rather than through a lossy integer cast, and by the same convention
/// the compliance journal's read and the export delivery use.
#[cfg(unix)]
fn same_entry(first: &rustix::fs::Stat, second: &rustix::fs::Stat) -> bool {
    first.st_dev == second.st_dev && first.st_ino == second.st_ino
}

/// A `rustix` error in the shape the rest of the crate reports a file-system
/// error, so a descriptor-relative failure and a path-based one read alike.
#[cfg(unix)]
fn io_error(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

/// The identity assertions of the confirming stat.
#[cfg(all(test, unix))]
#[path = "prune_delete_dir_tests.rs"]
mod tests;
