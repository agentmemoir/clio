// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore pre-flight: proving each
//! directory a restore writes into is writable and has room for that
//! directory's whole footprint before a single snapshot is taken.
//!
//! ## Owns
//! - `SpaceFootprint`, one directory's requirement, and `preflight_restore`,
//!   which sums the footprints that share a directory and checks each
//!   directory once for that total
//! - `SpaceCheck`, the injectable check, and `DiskSpace`, the production one
//! - `DF_PATHS`, the fixed absolute paths `df` is looked for, the descriptor
//!   pinned from the first of them that is a regular file, the report parser,
//!   and the statistics answer used where no descriptor can be pinned
//!
//! ## Does not own
//! - The swap itself (see `restore_swap`)
//! - Directory resolution (see `clio-config`) or the snapshot's own size
//!   estimate (see the create command)
//!
//! ## Boundary
//!
//! Changes to how a restore proves room or writability should normally require
//! changes here. Changes to what a restore replaces should not.
//!
//! Keep this module focused on the restore pre-flight.
//!
//! # Why `df` and not a syscall
//!
//! Free space is not available through the Rust standard library, and the
//! workspace denies `unsafe` outside the one sqlite-vec registration site. The
//! POSIX `df -P -k` report is the portable answer: the second line's fourth
//! column is the available space in 1024-byte blocks, on Linux, on macOS, and in
//! BusyBox. It is the primary answer. The file-system statistics behind
//! `rustix::fs::statvfs` are read only where no program can be pinned at all, so
//! a host whose `df` cannot be executed is still answered from its own
//! statistics instead of from a guess. A host where neither answers is reported
//! as a failure to check, never as "there is room".
//!
//! `df` is never resolved through `PATH`. It is looked for at the absolute paths
//! in `DF_PATHS` and nowhere else, because this pre-flight is a safety gate: the
//! process environment must not get to choose the program that decides whether a
//! restore is allowed to proceed. A `PATH` entry planted by anything the user
//! happens to run must not be able to answer "there is plenty of room".
//!
//! Nor is the chosen path resolved a second time. The first candidate that is a
//! regular file is opened once and judged through that descriptor, and the child
//! is executed from the descriptor, so replacing the path between the check and
//! the exec cannot make a different program run. `argv[0]` stays the candidate
//! path, which is what BusyBox reads when it picks an applet.

use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// One directory's restore footprint.
///
/// Several footprints may name the same directory. They are then staged side by
/// side and both exist on disk until the swap, so what the directory has to
/// hold is their sum, not either one alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceFootprint {
    /// The directory the bytes land in.
    pub dir: PathBuf,
    /// How many bytes must fit there.
    pub bytes: u64,
    /// What the bytes are, for the refusal message.
    pub label: &'static str,
}

/// One directory's whole requirement: every footprint that lands there, summed.
#[derive(Debug, PartialEq, Eq)]
struct DirectoryFootprint<'a> {
    /// The directory every footprint in the group lands in.
    dir: &'a Path,
    /// The sum of the group's bytes. Saturating, so an absurd total refuses
    /// rather than wrapping into a small number a check would accept.
    bytes: u64,
    /// What the bytes are, in input order, for the refusal message.
    labels: Vec<&'static str>,
}

/// Group the footprints that share a directory and sum each group.
///
/// Groups keep first-seen order, so directories are checked in the order the
/// caller's footprints first mention them. The input is not sorted or copied:
/// the groups borrow their directories from the caller's slice, and the only
/// allocation is the group list itself. A linear scan is right here because a
/// restore builds at most three footprints, so at most three groups.
fn group_by_directory(footprints: &[SpaceFootprint]) -> Vec<DirectoryFootprint<'_>> {
    let mut groups: Vec<DirectoryFootprint<'_>> = Vec::new();
    for footprint in footprints {
        match groups.iter_mut().find(|group| group.dir == footprint.dir) {
            Some(group) => {
                group.bytes = group.bytes.saturating_add(footprint.bytes);
                group.labels.push(footprint.label);
            }
            None => groups.push(DirectoryFootprint {
                dir: &footprint.dir,
                bytes: footprint.bytes,
                labels: vec![footprint.label],
            }),
        }
    }
    groups
}

/// Join a group's labels into one phrase: a single label is returned unchanged,
/// and several are joined in input order with commas and a final "and".
fn join_labels(labels: &[&'static str]) -> String {
    match labels.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_owned(),
        Some((last, head)) => format!("{} and {last}", head.join(", ")),
    }
}

/// Proves one directory can hold that directory's whole footprint.
///
/// The production implementation is [`DiskSpace`]; tests inject a check with a
/// fixed ceiling so an insufficient-space refusal is exercised without filling
/// a real filesystem.
pub trait SpaceCheck {
    /// Prove `dir` is writable and can hold `bytes` more data.
    ///
    /// `bytes` is the total every footprint in that directory needs, already
    /// summed by [`preflight_restore`], so a check is called once per directory
    /// with that directory's whole requirement. The production check therefore
    /// creates its probe file once per directory, not once per footprint.
    ///
    /// # Errors
    /// The directory is not writable, the filesystem does not have room, or
    /// the free space could not be determined.
    fn can_hold(&self, dir: &Path, bytes: u64) -> Result<(), AmError>;
}

/// The production check: one writability probe per directory, then the
/// filesystem's own free space report for that directory's whole footprint.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskSpace;

impl SpaceCheck for DiskSpace {
    fn can_hold(&self, dir: &Path, bytes: u64) -> Result<(), AmError> {
        probe_writable(dir)?;
        let available = available_bytes(dir)?;
        if available >= bytes {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::QuotaExceeded,
            format!(
                "{} bytes are available in {}, but {bytes} bytes are needed",
                available,
                dir.display()
            ),
        ))
    }
}

/// Check every directory once, in first-seen order, naming what does not fit.
///
/// Footprints that share a directory are summed and checked together, because a
/// restore stages them side by side and both are on disk until the swap.
/// Checking them one at a time would let two members each fit while the pair
/// does not, and the second one would then fail to stage after the pre-flight
/// had already said yes.
///
/// # Errors
/// The first directory that is not writable or has no room for its whole
/// footprint, with a message naming every member of that group and the
/// directory. The code is the check's own code, so a full directory is still
/// `QuotaExceeded` and a directory that will not accept writes is still
/// `InvalidArgument`.
pub fn preflight_restore(
    footprints: &[SpaceFootprint],
    check: &dyn SpaceCheck,
) -> Result<(), AmError> {
    for group in group_by_directory(footprints) {
        check.can_hold(group.dir, group.bytes).map_err(|error| {
            AmError::new(
                error.code,
                format!(
                    "{} cannot be placed in {}: {}",
                    join_labels(&group.labels),
                    group.dir.display(),
                    error.message
                ),
            )
        })?;
    }
    Ok(())
}

/// Create and remove one uniquely named empty file, proving the directory
/// accepts writes.
fn probe_writable(dir: &Path) -> Result<(), AmError> {
    if !dir.is_dir() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{} is not a directory", dir.display()),
        ));
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let probe = dir.join(format!(
        ".clio-restore-space-{}-{nanos}.tmp",
        std::process::id()
    ));
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("the directory {} is not writable: {error}", dir.display()),
        )),
    }
}

/// The filesystem's available bytes for `dir`.
///
/// A `df` report is used only when a descriptor can be pinned for it; without
/// that precondition the answer comes from the statistics and nothing is run.
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

/// A host that does not name a descriptor as a path answers from the statistics.
#[cfg(not(unix))]
fn fd_link_available() -> bool {
    false
}

/// The available bytes on `dir`'s filesystem, from its statistics: the answer
/// when no descriptor can be pinned, so no path decides anything.
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

/// A host with neither a pinned `df` nor file-system statistics answers nothing.
#[cfg(not(unix))]
fn available_bytes_without_df(dir: &Path) -> Result<u64, AmError> {
    Err(AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot check free space in {}: `df` failed: {}",
            dir.display(),
            missing_df_error(&DF_PATHS)
        ),
    ))
}

/// Run one `df` and read its report.
///
/// `run` is injected so every failure arm — `df` missing, `df` failing, an
/// unreadable report — is testable without breaking the host's `df`.
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

/// The absolute paths `df` is looked for, in order: `/bin/df` is macOS and
/// BusyBox, `/usr/bin/df` is the merged-`/usr` Linux, NixOS, and Gentoo layout,
/// and the `sbin` pair covers hosts that split `sbin` out of `usr`.
///
/// `PATH` is deliberately absent. The pre-flight is a gate, so the program that
/// answers it is fixed here rather than searched for.
const DF_PATHS: [&str; 4] = ["/bin/df", "/usr/bin/df", "/usr/sbin/df", "/sbin/df"];

/// The directory that names an open descriptor as a path this host can execute.
/// Its presence is the precondition for running `df` at all.
const FD_LINK_DIR: &str = "/proc/self/fd";

/// The `df` this host pins, from [`DF_PATHS`] only, without running anything.
///
/// Only the tests call this; production reaches the same pin through [`pin_df`].
#[cfg(all(test, unix))]
fn resolve_df() -> Option<PathBuf> {
    pin_df(&DF_PATHS).ok().map(|pinned| pinned.source)
}

/// A host that cannot name a descriptor has no `df` to resolve.
#[cfg(all(test, not(unix)))]
fn resolve_df() -> Option<PathBuf> {
    None
}

/// The `df` this host provides from `candidates`, kept open for the run.
///
/// # Errors
/// None of `candidates` is a regular file that can be opened; the message names
/// every path that was tried and never `PATH`.
#[cfg(unix)]
fn pin_df(candidates: &[&str]) -> std::io::Result<PinnedDf> {
    pin_df_from(candidates)?.ok_or_else(|| missing_df_error(candidates))
}

/// A `df` candidate whose type was checked through its own descriptor.
#[cfg(unix)]
struct PinnedDf {
    /// Held open for the run, so the executed descriptor is the inspected one.
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
/// blocking the open. The descriptor stays inheritable; the child executes it.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn pin_df_from(candidates: &[&str]) -> std::io::Result<Option<PinnedDf>> {
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
        return Ok(Some(PinnedDf {
            program: PathBuf::from(format!("{FD_LINK_DIR}/{}", fd.as_raw_fd())),
            source: PathBuf::from(*candidate),
            fd,
        }));
    }
    Ok(None)
}

/// The failure for a host with no `df` at any of the paths that were tried.
fn missing_df_error(candidates: &[&str]) -> std::io::Error {
    let tried = candidates.join(", ");
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no `df` at any of these absolute paths: {tried}"),
    )
}

/// Build and run `df -P -k <dir>` from the descriptor pinned for it.
///
/// The candidate is opened once and inspected through that descriptor, and the
/// child is executed from the descriptor, so the program whose type was checked
/// is the program that runs. The path is never resolved a second time, and the
/// descriptor is made inheritable first, because the child executes it.
#[cfg(unix)]
fn df_output(dir: &Path) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::CommandExt as _;

    let pinned = pin_df(&DF_PATHS)?;
    if rustix::io::fcntl_getfd(&pinned.fd)?.contains(rustix::io::FdFlags::CLOEXEC) {
        rustix::io::fcntl_setfd(&pinned.fd, rustix::io::FdFlags::empty())?;
    }
    std::process::Command::new(&pinned.program)
        .arg0(&pinned.source)
        .args(["-P", "-k"])
        .arg(dir)
        .output()
}

/// A host with no descriptor path executes nothing, so there is no report.
#[cfg(not(unix))]
fn df_output(_dir: &Path) -> std::io::Result<std::process::Output> {
    Err(missing_df_error(&DF_PATHS))
}

/// The available-space column of a POSIX `df -P -k` report: the header line is
/// skipped and the second line's fourth column is the count in 1024-byte
/// blocks. Split out so both output shapes can be tested without a filesystem.
fn parse_available_kb(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(test)]
#[path = "restore_space_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "restore_space_df_tests.rs"]
mod df_tests;
