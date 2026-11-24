// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for delivering an export destination through a
//! held directory descriptor: opening the trusted export root once, walking the
//! destination's parent components from it without following a link, and
//! creating and linking the delivered name relative to that descriptor.
//!
//! ## Owns
//! - The descriptor walk from the export root to the destination's parent
//! - The fail-closed refusal of a parent that leaves the root or is not a
//!   plain directory component
//! - The final-component classification, the link, the temp cleanup, and the
//!   directory flush, all relative to the held descriptor
//!
//! ## Does not own
//! - The delivery sequence, the custody rows, and the staging (`export_custody`)
//! - The managed temp name, the owner-only open, and the io-error mapping
//!   (`export_custody_files`)
//! - The confinement gate that proves the destination is inside the root
//!   (`clio-mcp`'s export destination gate)
//!
//! ## Boundary
//!
//! Changes to how a delivery reaches its destination directory should normally
//! require changes here. Changes to what an export contains, or to how the
//! custody rows are settled, should not.
//!
//! Why the descriptor: checking a path and then linking through that path lets
//! an actor swap an ancestor between the two moments, which redirects the link
//! outside the export root. Walking once from a held directory descriptor and
//! then linking two bare names in that same descriptor makes the check and the
//! use share one inode, so no ancestor can change under the link.
//!
//! Unix only: the operations are POSIX `openat`/`linkat`/`unlinkat`, and the
//! delivery sequence keeps its path-based form on other platforms.

use super::export_custody_files::{errno_error, write_destination_temp_at};
use super::{DeliveryFailure, DeliveryRefusal};
use clio_types::{AmError, ErrorCode};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode, fsync, linkat, open, openat, statat};
use rustix::io::Errno;
use std::ffi::OsString;
use std::os::fd::OwnedFd;
use std::path::{Component, Path};

/// The refusal every destination parent that leaves the export root receives.
const OUTSIDE: &str = "export destination leaves the export root";

/// Deliver `bytes` to `destination` under `root` through a held descriptor.
///
/// The root is opened once, the destination's parent components are walked from
/// it with `NOFOLLOW`, and the temp and the delivered name are both bare names
/// in the resulting descriptor. The final component is classified in that same
/// descriptor, so an existing file refuses as an existing destination and a
/// link, a directory, or any other non-regular name refuses as a non-regular
/// destination — both from the inode the link itself would see.
pub(super) fn deliver_relative(
    root: &Path,
    destination: &Path,
    action_id: &str,
    bytes: &[u8],
) -> Result<(), DeliveryFailure> {
    let dir = confined_dir(root, destination)?;
    let Some(dest_name) = destination.file_name() else {
        return Err(before_write(
            ErrorCode::InvalidArgument,
            "export destination names no file",
        ));
    };
    match statat(&dir, dest_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode as RawMode) != FileType::RegularFile => {
            return Err(DeliveryFailure::Refused(
                DeliveryRefusal::DestinationNotRegularFile,
            ));
        }
        Ok(_) | Err(Errno::NOENT) => {}
        Err(err) => {
            return Err(DeliveryFailure::BeforeWrite(errno_error(
                "inspect",
                destination,
                err,
            )));
        }
    }
    let tmp_name = write_destination_temp_at(&dir, destination, action_id, bytes)?;
    if let Err(err) = linkat(
        &dir,
        tmp_name.as_os_str(),
        &dir,
        dest_name,
        AtFlags::empty(),
    ) {
        // The link did not happen, so the delivery fails before the write and
        // the destination is untouched.
        let _ = rustix::fs::unlinkat(&dir, tmp_name.as_os_str(), AtFlags::empty());
        if err == Errno::EXIST {
            return Err(DeliveryFailure::Refused(DeliveryRefusal::DestinationExists));
        }
        return Err(DeliveryFailure::BeforeWrite(errno_error(
            "link",
            destination,
            err,
        )));
    }
    // The destination now names the delivered bytes; the temp name is a second
    // link to the same file, so removing it is best-effort cleanup. If a crash
    // lands before the removal, the temp sits at a managed name that
    // reconciliation sweeps.
    let _ = rustix::fs::unlinkat(&dir, tmp_name.as_os_str(), AtFlags::empty());
    // The held descriptor is a real `RDONLY` directory, so it can be flushed;
    // the entry exists either way, so a failure here is an after-write failure.
    fsync(&dir).map_err(|err| DeliveryFailure::AfterWrite(errno_error("sync", root, err)))?;
    Ok(())
}

/// Open the destination's parent directory from the export root, one component
/// at a time, without following a link at any step.
///
/// Fails closed: a component that is not a plain name, a parent that does not
/// strip against the root, and a component that cannot be opened as a plain
/// directory all refuse before any write. The raw root is what the parent is
/// stripped against, never its canonical form, because the delivered path is
/// the caller's raw candidate.
fn confined_dir(root: &Path, destination: &Path) -> Result<OwnedFd, DeliveryFailure> {
    let names = relative_parent(root, destination)?;
    let mut dir = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| DeliveryFailure::BeforeWrite(errno_error("open the export root", root, err)))?;
    for name in names {
        dir = openat(
            &dir,
            name.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|err| {
            DeliveryFailure::BeforeWrite(errno_error(
                "open the export destination parent",
                destination,
                err,
            ))
        })?;
    }
    Ok(dir)
}

/// The bare components from `root` down to the destination's parent.
///
/// A destination directly in the root has no components, so the link lands in
/// the root descriptor itself. A `.` component is skipped, so a `./nested`
/// parent still strips; a `..`, a root, or a prefix component, or a parent that
/// does not strip against the root at all, refuses.
fn relative_parent(root: &Path, destination: &Path) -> Result<Vec<OsString>, DeliveryFailure> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| before_write(ErrorCode::InvalidArgument, "destination has no parent"))?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| before_write(ErrorCode::InvalidArgument, OUTSIDE))?;
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => names.push(name.to_os_string()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(before_write(ErrorCode::InvalidArgument, OUTSIDE));
            }
        }
    }
    Ok(names)
}

/// A refusal that happened before any write.
fn before_write(code: ErrorCode, message: &str) -> DeliveryFailure {
    DeliveryFailure::BeforeWrite(AmError::new(code, message))
}

#[cfg(test)]
#[path = "export_custody_link_tests.rs"]
mod export_custody_link_tests;
