// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the free-space gate a verify pass runs before
//! it stages the payload beside the archive.
//!
//! ## Owns
//! - `require_room`: the refusal when the staging directory cannot hold the
//!   payload the manifest records
//! - The `df` candidate list, the descriptor that is pinned from it, the report
//!   parser, and the file-system-statistics answer used when no descriptor can
//!   be pinned
//!
//! ## Does not own
//! - Staging or removing the payload file (see `staging`)
//! - The archive walk that decides what to stage (see `verify`)
//! - Restore's own pre-flight, which plans several members at once (see
//!   `clio-store`)
//!
//! ## Boundary
//!
//! Changes to how much room a verify pass needs should normally require changes
//! here. Changes to when a member is staged should not.
//!
//! Keep this module focused on one free-space question.

use clio_types::{AmError, ErrorCode};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// The absolute paths `df` is looked for, in order: `/bin/df` is macOS and
/// BusyBox, `/usr/bin/df` is the merged-`/usr` Linux, NixOS, and Gentoo layout,
/// and the `sbin` pair covers hosts that split `sbin` out of `usr`.
///
/// `PATH` is deliberately absent. The check is a gate, so the program that
/// answers it is fixed here rather than searched for.
///
/// The restore pre-flight in `clio-store` (`restore_space`) holds the same
/// path list and report parser. The copy stays here so the backup container
/// crate does not depend on the store crate; keep the two in step, and change
/// both when a host layout or the report shape changes.
const DF_PATHS: [&str; 4] = ["/bin/df", "/usr/bin/df", "/usr/sbin/df", "/sbin/df"];

/// The directory that names an open descriptor as a path this host can
/// execute. Its presence is the precondition for running `df` at all.
const FD_LINK_DIR: &str = "/proc/self/fd";

/// Refuse when `dir` cannot hold `bytes` of payload.
///
/// # Errors
/// `QuotaExceeded` when the filesystem reports less free space than the
/// payload needs. `Internal` when no free-space answer can be read: a check
/// that could not run must not answer "there is room".
pub(crate) fn require_room(dir: &Path, bytes: u64) -> Result<(), AmError> {
    if bytes == 0 {
        return Ok(());
    }
    let available = available_bytes(dir)?;
    if available < bytes {
        return Err(AmError::new(
            ErrorCode::QuotaExceeded,
            format!(
                "staging the payload needs {bytes} bytes in {}, but only {available} bytes are \
                 free there",
                dir.display()
            ),
        ));
    }
    Ok(())
}

/// The filesystem's available bytes for `dir`.
///
/// A `df` report is used only when a descriptor can be pinned for it. Without
/// that precondition the answer comes from the file-system statistics, so no
/// path is ever executed.
fn available_bytes(dir: &Path) -> Result<u64, AmError> {
    if fd_link_available() {
        return run_available_bytes(dir, &df_output);
    }
    available_bytes_without_df(dir)
}

/// Whether a descriptor can be named as a path this host will execute.
#[cfg(unix)]
fn fd_link_available() -> bool {
    Path::new(FD_LINK_DIR).is_dir()
}

/// A host that does not name a descriptor as a path cannot run `df` this way,
/// so it answers from the file-system statistics instead.
#[cfg(not(unix))]
fn fd_link_available() -> bool {
    false
}

/// The available bytes on `dir`'s filesystem, from its statistics.
///
/// This is the answer when no descriptor can be pinned, so the gate never hands
/// its decision to a path that was checked before it was used.
#[cfg(unix)]
fn available_bytes_without_df(dir: &Path) -> Result<u64, AmError> {
    let space = rustix::fs::statvfs(dir).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `statvfs` failed: {error}",
                dir.display()
            ),
        )
    })?;
    Ok(space.f_bavail.saturating_mul(space.f_frsize))
}

/// A host with neither a pinned `df` nor file-system statistics answers
/// nothing, and must not guess.
#[cfg(not(unix))]
fn available_bytes_without_df(dir: &Path) -> Result<u64, AmError> {
    Err(AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot check free space in {}: `df` failed: {}",
            dir.display(),
            missing_df_error()
        ),
    ))
}

/// Run one `df` and read its report.
///
/// `run` is injected so every failure arm is testable without breaking the
/// host's `df`.
fn run_available_bytes(
    dir: &Path,
    run: &dyn Fn(&Path) -> std::io::Result<std::process::Output>,
) -> Result<u64, AmError> {
    let output = run(dir).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `df` failed: {error}",
                dir.display()
            ),
        )
    })?;
    if !output.status.success() {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `df` exited with {}",
                dir.display(),
                output.status
            ),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_available_kb(&text)
        .map(|kb| kb.saturating_mul(1024))
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot check free space in {}: `df` printed no readable available-space \
                     column",
                    dir.display()
                ),
            )
        })
}

/// Build and run `df -P -k <dir>` from the descriptor pinned for it.
///
/// The candidate is opened once and inspected through that descriptor, and the
/// child is executed from the descriptor, so the program whose type was checked
/// is the program that runs. The path is never resolved a second time.
#[cfg(unix)]
fn df_output(dir: &Path) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::CommandExt as _;

    let pinned = pin_df_from(&DF_PATHS)?.ok_or_else(missing_df_error)?;
    inherit_for_child(&pinned.fd)?;
    std::process::Command::new(&pinned.program)
        .arg0(&pinned.source)
        .arg("-P")
        .arg("-k")
        .arg(dir)
        .output()
}

/// A host with no descriptor path executes nothing, so the gate cannot answer
/// from `df` there.
#[cfg(not(unix))]
fn df_output(_dir: &Path) -> std::io::Result<std::process::Output> {
    Err(missing_df_error())
}

/// A `df` candidate whose type was checked through its own descriptor.
#[cfg(unix)]
struct PinnedDf {
    /// Held open for the whole run, so the descriptor the child executes is the
    /// one that was inspected.
    fd: rustix::fd::OwnedFd,
    /// The descriptor's own path, which is the exec target.
    program: PathBuf,
    /// The candidate path that was opened, which stays `argv[0]`.
    source: PathBuf,
}

/// Pin the first candidate that is a regular file, judged by its descriptor.
///
/// A candidate that cannot be opened is skipped, and so is one that is not a
/// regular file. `O_NONBLOCK` is deliberate: it stops a planted FIFO from
/// blocking the open. The descriptor is left inheritable, because the child
/// executes it.
///
/// The `Result` stays unused because opening a candidate is file-system work
/// and the caller reads the pin with `?` beside its own file-system calls. A
/// candidate that cannot be opened is skipped, never reported.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn pin_df_from(candidates: &[&str]) -> std::io::Result<Option<PinnedDf>> {
    use std::os::fd::AsRawFd as _;

    for candidate in candidates {
        let Ok(fd) = rustix::fs::open(
            *candidate,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        ) else {
            continue;
        };
        let Ok(stat) = rustix::fs::fstat(&fd) else {
            continue;
        };
        if rustix::fs::FileType::from_raw_mode(stat.st_mode as rustix::fs::RawMode)
            != rustix::fs::FileType::RegularFile
        {
            continue;
        }
        return Ok(Some(PinnedDf {
            program: PathBuf::from(format!("{FD_LINK_DIR}/{}", fd.as_raw_fd())),
            source: PathBuf::from(*candidate),
            fd,
        }));
    }
    Ok(None)
}

/// Keep the pinned descriptor inheritable, so the child holds the descriptor it
/// is executed from.
#[cfg(unix)]
fn inherit_for_child(fd: &rustix::fd::OwnedFd) -> std::io::Result<()> {
    if rustix::io::fcntl_getfd(fd)?.contains(rustix::io::FdFlags::CLOEXEC) {
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::empty())?;
    }
    Ok(())
}

/// The failure for a host with no `df` at any of the fixed paths.
fn missing_df_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "no `df` at any of these absolute paths: {}",
            DF_PATHS.join(", ")
        ),
    )
}

/// The available-space column of a POSIX `df -P -k` report: the header line is
/// skipped and the second line's fourth column is the count in 1024-byte
/// blocks.
fn parse_available_kb(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(test)]
#[path = "free_space_tests.rs"]
mod tests;
