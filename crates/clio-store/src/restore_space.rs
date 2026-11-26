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
//! - `DF_PATHS`, the fixed absolute paths `df` is looked for, the report
//!   parser, and the free-space answer that calls the pinned run
//!
//! ## Does not own
//! - Pinning a `df` candidate, running its descriptor, and the statistics answer
//!   used where no descriptor can be pinned (see `restore_space_pin`)
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
//! BusyBox. It is the answer wherever a `df` descriptor can be pinned and
//! executed.
//!
//! Three hosts do not get that answer, and none of them is told there is room:
//!
//! - A unix host that does not name an open descriptor as a path — no procfs, so
//!   no `/proc/self/fd` — answers from `rustix::fs::statvfs` and runs no `df` at
//!   all. macOS is such a host.
//! - A non-unix target executes nothing and reports the check as a failure to
//!   run, with the same "no `df` at any of these absolute paths" refusal a unix
//!   host with no `df` at any of them gets. It has no statistics to read instead.
//! - A host where the pinned descriptor cannot be executed is a failure to run as
//!   well: the pre-flight does not silently downgrade to a figure nothing
//!   checked, so a confined profile that blocks the exec fails closed.
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

use self::restore_space_pin::{available_bytes_with, df_output_from, fd_link_available};

#[path = "restore_space_pin.rs"]
mod restore_space_pin;

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
    available_bytes_with(dir, fd_link_available(), &df_output)
}

/// Build and run `df -P -k <dir>` from a descriptor pinned at one of this
/// crate's fixed candidate paths.
fn df_output(dir: &Path) -> std::io::Result<std::process::Output> {
    df_output_from(dir, &DF_PATHS)
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
///
/// The verify pass's free-space gate in `clio-backup` (`free_space`) holds its
/// own copy of this list, of the report parser below, and of the pin in
/// `restore_space_pin`. The copies stay separate so the backup container crate
/// does not depend on the store crate; keep them in step, and change both when a
/// host layout, the report shape, or the pin changes.
const DF_PATHS: [&str; 4] = ["/bin/df", "/usr/bin/df", "/usr/sbin/df", "/sbin/df"];

/// The `df` this host pins, from [`DF_PATHS`] only, without running anything.
///
/// Only the tests call this; production reaches the same pin through
/// `restore_space_pin`'s `df_output_from`.
#[cfg(all(test, unix))]
fn resolve_df() -> Option<PathBuf> {
    use self::restore_space_pin::pin_df;

    pin_df(&DF_PATHS).ok().map(|pinned| pinned.source)
}

/// A host that cannot name a descriptor has no `df` to resolve.
#[cfg(all(test, not(unix)))]
fn resolve_df() -> Option<PathBuf> {
    None
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

#[cfg(test)]
#[path = "restore_space_pin_tests.rs"]
mod pin_tests;
