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
//! - The fail-closed refusal other platforms report instead
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
//! Unix only: the calls are POSIX `openat`/`fstatat`/`unlinkat`, so an entry a
//! run inspected is confirmed by device and inode before it is unlinked. A
//! platform with no descriptor to hold and no inode identity to compare cannot
//! confirm that, so a confirmed removal there is refused and nothing is removed
//! (see `remove`). A report-only run still inspects by path on every platform.
//!
//! Keep this module focused on the file-system mechanics of one candidate.

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
/// after `unlinkat` returned success, from the inspection's own size, and only
/// when that inspection saw one link to the file: a file another hard link still
/// holds frees no blocks, so the entry is reported removed and no byte is
/// credited. A size that does not fit a byte count is a failure, never a zero.
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
    // The one moment a test may act in, so the two refusal arms below are
    // guarded by a case and not by hope. A test build only: no other build has
    // the module, the hook, or this call.
    #[cfg(all(test, unix))]
    gap::run(&held.dir, file_name);
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
            // A size the file system reports but cannot express as a byte count
            // is a failure, never a silent zero: a run must not report a removal
            // it cannot measure.
            let Ok(bytes) = u64::try_from(inspected.st_size) else {
                outcome.state = CandidateState::Failed;
                outcome.detail = Some(format!(
                    "removing {}: its size {} does not fit a byte count",
                    candidate.path.display(),
                    inspected.st_size
                ));
                return outcome;
            };
            // One link means the unlink freed these blocks. More than one means
            // the entry was removed from this directory while another name still
            // holds the file, so no byte was freed and none may be credited: the
            // entry is gone, its bytes are not.
            if inspected.st_nlink == 1 {
                outcome.reclaimed_bytes = bytes;
            } else {
                outcome.detail = Some(format!(
                    "{} was removed, but another link to it still holds its {bytes} bytes",
                    candidate.path.display()
                ));
            }
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
/// This platform has no `openat`/`statat`/`unlinkat` and no device-and-inode
/// comparison, so it cannot confirm that the entry a run removes is the entry it
/// inspected. The path is inspected once with `symlink_metadata`, which does not
/// follow a link, so a symbolic link and a directory are still refused with the
/// same report as on unix. A name that is still a plain file after that is then
/// refused rather than removed: the check-then-use gap between the inspection and
/// the `remove_file` cannot be closed here, so the removal is not taken.
#[cfg(not(unix))]
pub(super) fn remove(
    _held: &BackupDir,
    candidate: &ArchiveRef,
    _file_name: &OsStr,
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
    refuse_unconfirmed(candidate, outcome)
}

/// Report a candidate this platform cannot confirm before removal: nothing is
/// removed and nothing is credited.
///
/// The state is `Failed`, not a skip, because the removal did not happen and a
/// report must not present a refusal as a legitimate skip. The detail names the
/// archive and says why, so a confirmed run that removes nothing is visible as a
/// failure and not as an empty success.
#[cfg(any(not(unix), test))]
fn refuse_unconfirmed(candidate: &ArchiveRef, mut outcome: CandidateOutcome) -> CandidateOutcome {
    outcome.state = CandidateState::Failed;
    outcome.detail = Some(format!(
        "cannot confirm {} before removing it on this platform, so it was not removed",
        candidate.path.display()
    ));
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

/// The one moment a test may act in: the gap between the inspection and the
/// identity confirm.
///
/// Without it the two refusal arms of the confirm would be reached by no test,
/// and the mechanism that closes the reported bug would be guarded by nothing.
/// It is a test build only, so a release build has no hook, no module, and no
/// call into one. The hook is thread local, so two tests running side by side do
/// not see each other's action, and the guard clears it on drop, so a failing
/// assertion cannot leave one set for the next test.
#[cfg(all(test, unix))]
pub(crate) mod gap {
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::os::fd::OwnedFd;

    /// What a test does in the gap: the held directory and the bare name.
    type Action = Box<dyn Fn(&OwnedFd, &OsStr)>;

    thread_local! {
        /// The action this thread's run takes in the gap, when one is set.
        static HOOK: RefCell<Option<Action>> = const { RefCell::new(None) };
    }

    /// Act in the gap, when this thread has set a hook. Otherwise do nothing.
    pub(crate) fn run(dir: &OwnedFd, name: &OsStr) {
        HOOK.with_borrow(|hook| {
            if let Some(action) = hook {
                action(dir, name);
            }
        });
    }

    /// Holds one action in the gap until this value is dropped.
    pub(crate) struct Hook;

    impl Drop for Hook {
        fn drop(&mut self) {
            HOOK.with_borrow_mut(|hook| *hook = None);
        }
    }

    /// Set this thread's action in the gap, cleared when the returned guard drops.
    pub(crate) fn set(action: impl Fn(&OwnedFd, &OsStr) + 'static) -> Hook {
        HOOK.with_borrow_mut(|hook| *hook = Some(Box::new(action)));
        Hook
    }
}

/// The identity assertions of the confirming stat.
#[cfg(all(test, unix))]
#[path = "prune_delete_dir_tests.rs"]
mod tests;
