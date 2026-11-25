// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for removing the sync key file: the unlink of the
//! key and its creation sidecar, and the identity check that lets a removal run
//! only while the path still names the file the caller read.
//!
//! ## Owns
//! - [`remove_entry`]: the unlink of the key file and its sidecar
//! - [`remove_key_holding`]: the same removal, guarded by the descriptor the read
//!   returned
//! - The device-and-inode comparison that decides whether two descriptors name one
//!   file
//!
//! ## Does not own
//! - How the key is read, created, or where it lives (see `sync_key_file`)
//! - The judged open that refuses a link, a directory, and a foreign owner (see
//!   `sync_key_gate`)
//!
//! ## Boundary
//!
//! Changes to what may be unlinked, or to when a removal is refused, should
//! normally require changes here. Changes to the key's bytes, its path, or its
//! sidecar contents should not.
//!
//! Keep this module focused on the removal of the key entry.
//!
//! # Why the removal asks the descriptor
//!
//! A removal that asks the path can be answered by one object and then unlink
//! another: an entry swapped between the archive write and the unlink would let a
//! rotation archive one key and delete a different one. The caller hands over the
//! descriptor its read returned, this module opens the path again through the
//! judged gate, and the two descriptors are compared by device and inode. Only a
//! path that still names the archived file loses it.

use std::fs;
use std::path::Path;

use clio_types::{AmError, ErrorCode};

use crate::sync_key_file::{open_optional, sidecar_path};

/// Remove the sync key file and its sidecar, succeeding when neither exists.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the entry when the filesystem refused the
/// removal.
pub(crate) fn remove_entry(path: &Path) -> Result<(), AmError> {
    for candidate in [
        path.to_path_buf(),
        sidecar_path(Some(path)).unwrap_or_default(),
    ] {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(remove_error(&candidate, &err)),
        }
    }
    Ok(())
}

/// Remove the sync key file only while `path` still names the object `held`.
///
/// The descriptor `held` is the one the read returned, so the caller archived the
/// bytes of that object and not of whatever the path shows now. This re-opens the
/// path through the same judged gate and compares the two descriptors. A link, a
/// directory, a file another user owns, or a file that is not owner-only is refused
/// by that gate before any comparison, and nothing is unlinked.
///
/// The comparison narrows the window between the read and the unlink. The unlink
/// still names the path, so an entry swapped after the comparison and before the
/// removal is a window this cannot close: stable Rust has no unlink that takes the
/// descriptor's identity.
///
/// `Ok(())` when the path holds nothing: the entry the caller archived is already
/// gone, so there is nothing left to remove.
///
/// # Errors
/// [`ErrorCode::Forbidden`] naming the path when the entry changed after it was
/// archived, or when the gate refused it; [`ErrorCode::Internal`] when the removal
/// failed.
pub(crate) fn remove_key_holding(path: &Path, held: &fs::File) -> Result<(), AmError> {
    let Some(current) = open_optional(path)? else {
        return Ok(());
    };
    if !names_the_same_file(held, &current) {
        return Err(changed_after_archive(path));
    }
    drop(current);
    remove_entry(path)
}

/// The filesystem identity of one open file: the device and inode it lives at.
#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl FileIdentity {
    /// The identity of the file `file` is open on, asked of its descriptor.
    fn of(file: &fs::File) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok(Self {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
}

/// Whether `current` is the very object `held` was opened on.
///
/// Two descriptors are compared by device and inode, never by path: the removal
/// that follows unlinks the name the path holds now, so a path swapped after the
/// read would otherwise delete an entry the archive never took. A descriptor whose
/// identity cannot be read is not confirmed as the archived file and answers
/// `false`, so the refusal is the safe side.
#[cfg(unix)]
fn names_the_same_file(held: &fs::File, current: &fs::File) -> bool {
    match (FileIdentity::of(held), FileIdentity::of(current)) {
        (Ok(held), Ok(current)) => held == current,
        _ => false,
    }
}

/// Other platforms report no descriptor identity through `std`, so this cannot tell
/// a replacement from the file that was read and answers `true`: the check is
/// Unix-only, and this is the stated gap.
#[cfg(not(unix))]
fn names_the_same_file(_held: &fs::File, _current: &fs::File) -> bool {
    true
}

/// The refusal a path that no longer names the archived key reports.
fn changed_after_archive(path: &Path) -> AmError {
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "the sync key file {} changed after it was archived; Clio has not removed it, \
             because the entry there is no longer the key the archive holds. Restore the archived \
             key at that path and run again",
            path.display()
        ),
    )
}

/// One key-material entry the filesystem refused to remove.
fn remove_error(path: &Path, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot remove the Clio sync key material at {}: {err}",
            path.display()
        ),
    )
}

#[cfg(test)]
#[path = "sync_key_file_remove_tests.rs"]
mod tests;
