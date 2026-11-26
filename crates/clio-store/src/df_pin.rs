// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for pinning the `df` the restore pre-flight
//! executes, running that descriptor, and the answer used when no descriptor can
//! be pinned.
//!
//! ## Owns
//! - `FD_LINK_DIR`, the directory that names an open descriptor as a path this
//!   host can execute, and whether this host provides one
//! - Pinning the first candidate that is a regular file, through its own
//!   descriptor, above the standard streams, and running that descriptor
//! - The file-system-statistics answer, and the chooser between it and `df`
//!
//! ## Does not own
//! - The candidate list, the report parser, and the pre-flight itself (see
//!   `restore_space`)
//! - The swap a restore performs (see `restore_swap`)
//!
//! ## Boundary
//!
//! Changes to how a `df` candidate is opened, judged, or executed should
//! normally require changes here. Changes to when the pre-flight runs should
//! not.
//!
//! Keep this module focused on one pinned `df` descriptor.
//!
//! This module owns its tests, declared below and kept in `df_pin_tests`.

use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// The directory that names an open descriptor as a path this host can execute.
/// Its presence is the precondition for running `df` at all.
#[cfg(unix)]
pub(super) const FD_LINK_DIR: &str = "/proc/self/fd";

/// Whether a descriptor can be named as a path this host will execute.
#[cfg(unix)]
pub(super) fn fd_link_available() -> bool {
    Path::new(FD_LINK_DIR).is_dir()
}

/// A non-unix target names no descriptor as a path, so nothing is executed
/// there.
#[cfg(not(unix))]
pub(super) fn fd_link_available() -> bool {
    false
}

/// The filesystem's available bytes for `dir`, chosen by `fd_link`.
///
/// `fd_link` is the host's answer to [`fd_link_available`], passed in rather
/// than read here, so both arms can be driven on any host. `run` is the `df`
/// that arm executes, injected for the same reason.
pub(super) fn available_bytes_with(
    dir: &Path,
    fd_link: bool,
    run: &dyn Fn(&Path) -> std::io::Result<std::process::Output>,
) -> Result<u64, AmError> {
    if fd_link {
        return super::run_available_bytes(dir, run);
    }
    available_bytes_without_df(dir)
}

/// The available bytes on `dir`'s filesystem, from its statistics.
///
/// This is the answer where no descriptor can be pinned at all, so the gate
/// never hands its decision to a path that was checked before it was used. It is
/// not a fallback for a descriptor that failed to execute: that is reported as a
/// failure to run, never silently replaced by a figure nothing checked.
#[cfg(unix)]
pub(super) fn available_bytes_without_df(dir: &Path) -> Result<u64, AmError> {
    let space = rustix::fs::statvfs(dir).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `statvfs` failed: {error}",
                dir.display()
            ),
        )
    })?;
    // A FUSE server can report no fragment size; the block size is then the only
    // usable figure, and multiplying every count by zero would refuse every
    // restore on a mount that has room.
    let fragment = if space.f_frsize == 0 {
        space.f_bsize
    } else {
        space.f_frsize
    };
    Ok(space.f_bavail.saturating_mul(fragment))
}

/// A target with neither a pinned `df` nor file-system statistics answers
/// nothing, and must not guess.
#[cfg(not(unix))]
pub(super) fn available_bytes_without_df(dir: &Path) -> Result<u64, AmError> {
    Err(AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot check free space in {}: `df` failed: {}",
            dir.display(),
            missing_df_error(&super::DF_PATHS)
        ),
    ))
}

/// A `df` candidate whose type was checked through its own descriptor.
#[cfg(unix)]
pub(super) struct PinnedDf {
    /// Held open for the run, so the executed descriptor is the inspected one.
    pub(super) fd: rustix::fd::OwnedFd,
    /// The descriptor's own path, which is the exec target.
    pub(super) program: PathBuf,
    /// The candidate path that was opened, which stays `argv[0]`.
    pub(super) source: PathBuf,
}

/// The `df` this host provides from `candidates`, kept open for the run.
///
/// # Errors
/// None of `candidates` is a regular file that can be opened; the message names
/// every path that was tried and never `PATH`.
#[cfg(unix)]
pub(super) fn pin_df(candidates: &[&str]) -> std::io::Result<PinnedDf> {
    pin_df_from(candidates).ok_or_else(|| missing_df_error(candidates))
}

/// Pin the first candidate that is a regular file, judged by its descriptor.
///
/// A candidate that cannot be opened is skipped, and so is one that is not a
/// regular file, and so is one whose descriptor cannot be moved above the
/// standard streams. `O_NONBLOCK` is deliberate: it stops a planted FIFO from
/// blocking the open.
///
/// The kept descriptor is always fd 3 or higher. A host may start with some of
/// its standard streams closed, so `open` can return 0, 1 or 2, and a child's
/// stdio is installed over exactly those slots: the descriptor the child would
/// execute from would then name a pipe instead of the candidate, and the exec
/// would fail. The duplicate carries `FD_CLOEXEC`, which [`inherit_for_child`]
/// clears, because the child inherits the descriptor it is executed from.
#[cfg(unix)]
pub(super) fn pin_df_from(candidates: &[&str]) -> Option<PinnedDf> {
    use rustix::fd::AsRawFd as _;
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, open};

    for candidate in candidates {
        let flags = OFlags::RDONLY | OFlags::NONBLOCK;
        let Ok(fd) = open(*candidate, flags, Mode::empty()) else {
            continue;
        };
        let Ok(stat) = fstat(&fd) else {
            continue;
        };
        if FileType::from_raw_mode(stat.st_mode as RawMode) != FileType::RegularFile {
            continue;
        }
        // A descriptor that cannot be moved is as unusable as one that could not
        // be opened, so the next candidate is tried and no error is invented.
        let Ok(child_fd) = rustix::io::fcntl_dupfd_cloexec(&fd, 3) else {
            continue;
        };
        drop(fd);
        return Some(PinnedDf {
            program: PathBuf::from(format!("{FD_LINK_DIR}/{}", child_fd.as_raw_fd())),
            source: PathBuf::from(*candidate),
            fd: child_fd,
        });
    }
    None
}

/// Keep the pinned descriptor inheritable, so the child holds the descriptor it
/// is executed from.
#[cfg(unix)]
pub(super) fn inherit_for_child(fd: &rustix::fd::OwnedFd) -> std::io::Result<()> {
    if rustix::io::fcntl_getfd(fd)?.contains(rustix::io::FdFlags::CLOEXEC) {
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::empty())?;
    }
    Ok(())
}

/// Build and run `df -P -k <dir>` from the first candidate that can be pinned.
///
/// The candidate is opened once and inspected through that descriptor, and the
/// child is executed from the descriptor, so the program whose type was checked
/// is the program that runs. The path is never resolved a second time.
#[cfg(unix)]
pub(super) fn df_output_from(
    dir: &Path,
    candidates: &[&str],
) -> std::io::Result<std::process::Output> {
    let pinned = pin_df(candidates)?;
    run_pinned_df(&pinned, dir)
}

/// Run `df -P -k <dir>` from a descriptor that was pinned before this call.
///
/// The exec target is the descriptor, never `pinned.source`: a path resolved here
/// could name a different file than the one whose type was checked.
#[cfg(unix)]
pub(super) fn run_pinned_df(
    pinned: &PinnedDf,
    dir: &Path,
) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::CommandExt as _;

    inherit_for_child(&pinned.fd)?;
    std::process::Command::new(&pinned.program)
        .arg0(&pinned.source)
        .args(["-P", "-k"])
        .arg(dir)
        .output()
}

/// A target that names no descriptor as a path executes nothing, so it has no
/// report to answer with.
#[cfg(not(unix))]
pub(super) fn df_output_from(
    _dir: &Path,
    candidates: &[&str],
) -> std::io::Result<std::process::Output> {
    Err(missing_df_error(candidates))
}

/// The failure for a host with no `df` at any of the paths that were tried.
pub(super) fn missing_df_error(candidates: &[&str]) -> std::io::Error {
    let tried = candidates.join(", ");
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no `df` at any of these absolute paths: {tried}"),
    )
}

#[cfg(test)]
#[path = "df_pin_tests.rs"]
mod tests;
