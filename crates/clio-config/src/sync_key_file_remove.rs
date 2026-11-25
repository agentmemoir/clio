// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for removing the sync key file: the unlink of the key and its creation sidecar, and the identity check that lets a removal run only while the path still names the file the caller read.
//!
//! ## Owns
//! - [`remove_entry`]: the explicit unlink of the key file and its sidecar
//! - [`remove_key_holding`]: the same removal, guarded by the descriptor the read returned and by the creation stamp the archive recorded
//! - The device-and-inode comparison that decides whether two descriptors name one file
//!
//! ## Does not own
//! - How the key is read, created, or where it lives (see `sync_key_file`)
//! - The judged open that refuses a link, a directory, and a foreign owner (see `sync_key_gate`)
//!
//! ## Boundary
//!
//! Changes to what may be unlinked, or to when a removal is refused, should normally require changes here. Changes to the key's bytes, its path, or its sidecar contents should not.
//!
//! Keep this module focused on the removal of the key entry.
//!
//! # Why the removal asks the descriptor
//!
//! A removal that asks the path can be answered by one object and then unlink another: an entry swapped between the archive write and the unlink would let a rotation archive one key and delete a different one. The caller hands over the descriptor its read returned, this module opens the path again through the judged gate, and the two descriptors are compared by device and inode. Only a path that still names the archived file loses it.
//!
//! # Why the sidecar is read again
//!
//! The sidecar describes whichever key was written last, and its name comes from the directory. A sidecar replaced after the archive describes a different key, so unlinking it on the strength of the key file would throw away the only creation record of the key that replaced the archived one. The removal therefore unlinks the sidecar only while it still returns the stamp the archive stored.

use std::fs;
use std::path::Path;

use clio_types::{AmError, ErrorCode};

use crate::sync_key_file::{open_optional, read_created, sidecar_path};

/// Remove the sync key file and its sidecar, succeeding when neither exists.
///
/// This is the explicit removal: the caller asked for the entry to go, so the sidecar goes with it. A rotation uses [`remove_key_holding`], which lets go of the sidecar only while it still describes the key the archive took.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the entry when the filesystem refused the removal.
pub(crate) fn remove_entry(path: &Path) -> Result<(), AmError> {
    remove_file(path)?;
    remove_sidecar(path)
}

/// Remove the sync key file only while `path` still names the object `held`, and its sidecar only while the record still carries the `created` stamp the archive stored.
///
/// The descriptor `held` is the one the read returned, so the caller archived the bytes of that object and not of whatever the path shows now. This re-opens the path through the same judged gate and compares the two descriptors. A link, a directory, a file another user owns, or a file that is not owner-only is refused by that gate before any comparison, and nothing is unlinked.
///
/// The comparison narrows the window between the read and the unlink. The unlink still names the path, so an entry swapped after the comparison and before the removal is a window this cannot close: stable Rust has no unlink that takes the descriptor's identity.
///
/// `created` is the creation stamp the archive recorded for `held`, or `None` when the archive recorded none. The sidecar is read again here and is unlinked only when it still returns that same stamp. A sidecar that names another key is left in place: it is the creation record of whatever replaced the key, and a later generation reuses it rather than starting from an unknown time.
///
/// `Ok(())` when the path holds nothing: the entry the caller archived is already gone, and only a sidecar that still matches the archive is let go of.
///
/// # Errors
/// [`ErrorCode::Forbidden`] naming the path when the removal cannot confirm the entry is the archived one, or when the gate refused it; [`ErrorCode::Internal`] when the sidecar cannot be read or a removal failed.
pub(crate) fn remove_key_holding(
    path: &Path,
    held: &fs::File,
    created: Option<&str>,
) -> Result<(), AmError> {
    let Some(current) = open_optional(path)? else {
        return remove_matching_sidecar(path, created);
    };
    if !names_the_same_file(held, &current) {
        return Err(refused_identity(path));
    }
    drop(current);
    let sidecar_matches = sidecar_matches(path, created)?;
    remove_file(path)?;
    if sidecar_matches {
        remove_sidecar(path)?;
    }
    Ok(())
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
/// Two descriptors are compared by device and inode, never by path: the removal that follows unlinks the name the path holds now, so a path swapped after the read would otherwise delete an entry the archive never took. A descriptor whose identity cannot be read is not confirmed as the archived file and answers `false`, so the refusal is the safe side.
#[cfg(unix)]
fn names_the_same_file(held: &fs::File, current: &fs::File) -> bool {
    match (FileIdentity::of(held), FileIdentity::of(current)) {
        (Ok(held), Ok(current)) => held == current,
        _ => false,
    }
}

/// Whether `current` is the very object `held` was opened on, which this platform cannot answer.
///
/// This platform reports no device or inode through `std`, so two descriptors cannot be compared. It answers `false` so a removal refuses instead of unlinking whatever the path holds: an identity a program cannot confirm is not an identity it may treat as the archived file.
#[cfg(not(unix))]
fn names_the_same_file(_held: &fs::File, _current: &fs::File) -> bool {
    false
}

/// Whether the sidecar still carries `created`; a missing sidecar does not match a recorded stamp.
fn sidecar_matches(path: &Path, created: Option<&str>) -> Result<bool, AmError> {
    Ok(read_created(Some(path))?.as_deref() == created)
}

/// Let go of the sidecar only while it still carries `created`.
fn remove_matching_sidecar(path: &Path, created: Option<&str>) -> Result<(), AmError> {
    if sidecar_matches(path, created)? {
        remove_sidecar(path)?;
    }
    Ok(())
}

/// Unlink the sidecar beside `path`, treating an absent sidecar as already removed.
fn remove_sidecar(path: &Path) -> Result<(), AmError> {
    match sidecar_path(Some(path)) {
        Some(sidecar) => remove_file(&sidecar),
        None => Ok(()),
    }
}

/// Unlink one entry, treating an absent entry as already removed.
fn remove_file(path: &Path) -> Result<(), AmError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(remove_error(path, &err)),
    }
}

/// The refusal a path whose identity the removal cannot confirm reports.
///
/// The message names the entry, because the entry is still there: the removal stopped before unlinking anything.
#[cfg(unix)]
fn refused_identity(path: &Path) -> AmError {
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "the sync key file {} changed after it was archived; Clio has not removed it, because the entry there is no longer the key the archive holds. Restore the archived key at that path and run again",
            path.display()
        ),
    )
}

/// The refusal this platform reports because it cannot compare two descriptors.
///
/// The removal cannot tell a replacement from the file that was read, so it stops and leaves the entry in place. A caller must never read an unconfirmed identity as success.
#[cfg(not(unix))]
fn refused_identity(path: &Path) -> AmError {
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "Clio cannot confirm that {} is still the sync key file it archived, because this platform does not report a file's identity to a program; Clio has left the entry in place. Remove it by hand after checking it is the key the archive holds",
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
