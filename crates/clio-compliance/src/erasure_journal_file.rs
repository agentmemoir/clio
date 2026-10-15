// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the owner-only file mechanics of the durable
//! erasure journal: reading it into a raw classified body and writing it with
//! the atomic sequence that survives a crash.
//!
//! ## Owns
//! - The raw read classification: absent, damaged, or bounded bytes
//! - The owner-only write: create-new temp, exact 0600, sync, rename, parent fsync
//! - The expected-pre-image guard the write checks immediately before the rename
//! - The same-directory temp name and the stale-temp removal
//!
//! ## Does not own
//! - The journal model, schema, and operations (`erasure_journal` owns those)
//! - Where the journal lives (`clio-config::resolve_journal_path`)
//!
//! ## Boundary
//!
//! Changes to how the journal bytes reach or leave the disk should normally
//! require changes here. Changes to the journal's shape or checks should not.
//!
//! Keep this module focused on the journal's file mechanics.
//!
//! # Why the temp is a sibling
//!
//! The canonical durable sequence is create temp on the same filesystem,
//! write, sync the file, rename it over the target, then sync the directory:
//! `rename(2)` is atomic but refuses across mounts, and a file sync does not
//! cover the directory entry the rename creates.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};

use crate::export_custody::export_custody_files::{fsync_dir, io_error, open_owner_only};

/// The largest journal body in bytes; a larger file is damaged, never parsed.
pub(crate) const MAX_JOURNAL_BYTES: u64 = 1 << 20;

/// The suffix of the same-directory temp the atomic write renames from.
const TEMP_SUFFIX: &str = ".erasure-journal-tmp";

/// The raw outcome of reading the owner-only journal path.
pub(crate) enum ReadBody {
    /// No file exists at the path.
    Absent,
    /// The path holds something this build will not read: a non-file, an
    /// empty or oversized body, group or other access, or an IO error.
    Damaged,
    /// The bounded body bytes.
    Bytes(Vec<u8>),
}

/// The destination pre-image an atomic write expects to replace.
pub(crate) enum ExpectedBody<'a> {
    /// No file may exist at the destination.
    Absent,
    /// The destination must still hold exactly these bytes.
    Bytes(&'a [u8]),
}

/// Read the journal at `path` into a raw body.
///
/// Every failure other than "file absent" classifies as [`ReadBody::Damaged`];
/// a caller never sees a widened, oversized, or unreadable file as bytes. The
/// path is observed without following links, then opened once, and the opened
/// descriptor is the source of truth: its type, mode, length, and identity
/// decide, and the body is read from that same descriptor.
pub(crate) fn read_body(path: &Path) -> ReadBody {
    // A symlink, or any other non-regular file, at the journal path fails
    // closed as damaged even when it points at bytes this build would accept.
    let observed = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return ReadBody::Absent,
        Err(_) => return ReadBody::Damaged,
    };
    if !trusted(&observed) {
        return ReadBody::Damaged;
    }
    let Ok(file) = fs::File::open(path) else {
        return ReadBody::Damaged;
    };
    let Ok(opened) = file.metadata() else {
        return ReadBody::Damaged;
    };
    // The descriptor must name the file the path observation saw, so a path
    // swapped between the two steps is refused instead of being followed.
    if !trusted(&opened) || !same_file(&observed, &opened) {
        return ReadBody::Damaged;
    }
    let mut bytes = Vec::new();
    let read = file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes);
    if read.is_err() || !bounded_len(bytes.len() as u64) {
        return ReadBody::Damaged;
    }
    ReadBody::Bytes(bytes)
}

/// Write `bytes` to `path` with the owner-only atomic sequence: same-directory
/// temp (create-new, 0600), write, sync, rename over the target, then fsync
/// the parent directory.
///
/// The destination is re-read with the same fail-closed classification as
/// [`read_body`] immediately before the rename and must still match
/// `expected`; a mismatch removes the temp and refuses instead of overwriting
/// a concurrent change. That guard narrows the race rather than eliminating
/// it — the comparison and the rename are not one step — so serializing
/// overlapping writers remains the caller's lock. A failed sync is a failure,
/// because bytes that did not reach the disk are not durable. A stale regular
/// temp left by a crash is removed first; a link or a directory at the temp
/// name is never followed or removed.
pub(crate) fn write_body(
    path: &Path,
    expected: &ExpectedBody<'_>,
    bytes: &[u8],
) -> Result<(), AmError> {
    if !bounded_len(bytes.len() as u64) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!(
                "the erasure journal body is {} bytes; the limit is {MAX_JOURNAL_BYTES}",
                bytes.len()
            ),
        ));
    }
    let dir = journal_dir(path);
    let temp = temp_path(path);
    if let Ok(meta) = fs::symlink_metadata(&temp)
        && meta.file_type().is_file()
    {
        fs::remove_file(&temp).map_err(|err| io_error("remove", &temp, &err))?;
    }
    let mut file = open_owner_only(&temp).map_err(|err| io_error("create", &temp, &err))?;
    force_owner_only(&file, &temp)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error("write", &temp, &err))?;
    if !destination_matches(path, expected) {
        let _ = fs::remove_file(&temp);
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "the erasure journal at {} changed concurrently; refusing to overwrite it",
                path.display()
            ),
        ));
    }
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_error("rename", path, &err));
    }
    fsync_dir(dir)
}

/// Whether `path` still holds `expected`, under the fail-closed read
/// classification: a symlink, a non-regular file, a widened mode, an empty or
/// oversized body, or an IO error is never a match.
fn destination_matches(path: &Path, expected: &ExpectedBody<'_>) -> bool {
    match (read_body(path), expected) {
        (ReadBody::Absent, ExpectedBody::Absent) => true,
        (ReadBody::Bytes(actual), ExpectedBody::Bytes(want)) => actual == *want,
        _ => false,
    }
}

/// Whether an observation is a regular, owner-only, bounded journal file.
fn trusted(meta: &fs::Metadata) -> bool {
    meta.is_file() && owner_only(meta) && bounded_len(meta.len())
}

/// Whether the opened descriptor still names the file the path observation saw.
#[cfg(unix)]
fn same_file(observed: &fs::Metadata, opened: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    observed.dev() == opened.dev() && observed.ino() == opened.ino()
}

/// Non-Unix platforms have no inode identity to compare; the descriptor's own
/// type, mode, and length checks are what remain.
#[cfg(not(unix))]
fn same_file(_observed: &fs::Metadata, _opened: &fs::Metadata) -> bool {
    true
}

/// Whether the file mode grants nothing to group or other.
#[cfg(unix)]
fn owner_only(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode().trailing_zeros() >= 6
}

/// Non-Unix platforms have no mode bits to check; the owner-only rule is a
/// no-op there.
#[cfg(not(unix))]
fn owner_only(_meta: &fs::Metadata) -> bool {
    true
}

/// Whether a body length is in the readable, writable range.
fn bounded_len(len: u64) -> bool {
    len > 0 && len <= MAX_JOURNAL_BYTES
}

/// The directory the journal rename and parent fsync stay inside.
fn journal_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The hidden same-directory temp sibling the atomic write renames from.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("erasure-journal.json");
    path.with_file_name(format!(".{name}{TEMP_SUFFIX}"))
}

/// Force the temp file to exactly `0600`: the `open` mode applies only at
/// creation and is masked by the process umask, so the mode is set again on
/// the opened file. An existing target keeps its own mode until the rename
/// replaces it, which is why the read side refuses a widened file.
#[cfg(unix)]
fn force_owner_only(file: &fs::File, path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|err| io_error("restrict", path, &err))
}

/// Non-Unix platforms have no mode bits to set; the owner-only rule is a
/// no-op there.
#[cfg(not(unix))]
fn force_owner_only(_file: &fs::File, _path: &Path) -> Result<(), AmError> {
    Ok(())
}

#[cfg(test)]
#[path = "erasure_journal_file_tests.rs"]
mod erasure_journal_file_tests;
