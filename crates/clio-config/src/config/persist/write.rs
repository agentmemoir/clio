// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for owner-only file writes and the owner-only
//! (0700) directory permission rule other crates call.
//!
//! ## Owns
//! - Atomic temp-file-plus-rename writes with 0600 permissions
//! - Temp files that are created new, so an entry already at the temp name is
//!   refused instead of followed
//! - The monotonic counter that keeps concurrent temp files distinct
//! - The pre-rename check that refuses to replace a different entry at the
//!   target path
//! - `restrict_dir`, the owner-only directory rule
//! - `create_private_dir`, which makes a missing directory owner-only
//!
//! ## Does not own
//! - What is written into the file (see `save`)
//! - The timestamped backup layout (see `backup`)
//! - The deployment file name itself (see `path`)
//!
//! ## Boundary
//!
//! Changes to how bytes reach disk or to the owner-only rule belong here.
//! Changes to the overlay contents should not require changes here.
//!
//! Keep this module focused on owner-only file and directory writes.

use super::path::DEPLOYMENT_CONFIG_FILE_NAME;
use crate::error::{AmError, ErrorCode};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter for temp-file names. PID plus nanos is not
/// enough on its own: two writers can read the same clock value, so the
/// counter guarantees distinct temp siblings even in the same nanosecond.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically, owner-only on Unix. The temp file
/// lives beside the target so the final rename stays on one filesystem. The
/// temp file is created new: an existing entry at that name, such as a planted
/// symlink, is refused rather than followed and truncated.
/// Crate-shared: the token store saves through the same writer.
///
/// # Errors
/// The temp file cannot be created, written, or synced, or the final rename
/// fails (the temp file is removed before returning).
pub fn write_private_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    write_private_atomic_to(path, &tmp, contents)
}

/// Write `contents` to `path` atomically, but run `check` after the temp file
/// is ready and before the rename. A failed check removes the temp file and
/// changes nothing at `path`.
///
/// # Errors
/// The I/O errors of [`write_private_atomic`], or the refusal `check` returns.
pub(crate) fn write_private_atomic_checked(
    path: &Path,
    contents: &str,
    check: impl FnOnce() -> Result<(), AmError>,
) -> Result<(), AmError> {
    let tmp = temp_sibling(path);
    write_atomic_to(path, &tmp, contents, check, |e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write deployment config {}: {e}", path.display()),
        )
    })
}

/// Write `contents` to `path` through the temp path `tmp`: create `tmp` new,
/// then rename it onto `path`. `tmp` is removed only when the rename fails.
///
/// # Errors
/// `AlreadyExists` when any entry already sits at `tmp`, including a symlink;
/// any create, write, or sync error on the temp file; or the rename error.
fn write_private_atomic_to(path: &Path, tmp: &Path, contents: &str) -> std::io::Result<()> {
    write_atomic_to(path, tmp, contents, || Ok(()), std::convert::identity)
}

/// The shared temp-create, check, rename, and cleanup body.
///
/// # Errors
/// An error from creating or writing `tmp`, the refusal `check` returns, or
/// the rename error, mapped through `io_failure`.
fn write_atomic_to<E>(
    path: &Path,
    tmp: &Path,
    contents: &str,
    check: impl FnOnce() -> Result<(), E>,
    io_failure: impl Fn(std::io::Error) -> E,
) -> Result<(), E> {
    create_new_private(tmp, contents).map_err(&io_failure)?;
    if let Err(refusal) = check() {
        let _ = fs::remove_file(tmp);
        return Err(refusal);
    }
    if let Err(e) = fs::rename(tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(io_failure(e));
    }
    Ok(())
}

pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(DEPLOYMENT_CONFIG_FILE_NAME);
    // PID + nanos + a monotonic counter keeps concurrent writers (threads or
    // processes) from clobbering each other's temp file before the rename.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        seq
    ))
}

#[cfg(unix)]
pub(super) fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // A pre-existing temp file keeps its old mode through `open`; force 0600.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(not(unix))]
pub(super) fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    fs::write(path, contents)
}

/// Create `path` as a new file and write `contents`, owner-only on Unix.
///
/// # Errors
/// `AlreadyExists` when any entry already sits at `path`, including a symlink.
/// `create_new` is `O_CREAT|O_EXCL`, so the open fails instead of following the
/// entry and truncating its target. A write or sync error removes the new file.
#[cfg(unix)]
pub(super) fn create_new_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let result = (|| -> std::io::Result<()> {
        // `mode` only applies to a created file; force 0600 on the handle anyway.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = result {
        // Only this call can have created `path`, so only this call removes it.
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// Create `path` as a new file and write `contents`.
///
/// # Errors
/// `AlreadyExists` when any entry already sits at `path`, including a symlink.
/// A write error removes the new file.
#[cfg(not(unix))]
pub(super) fn create_new_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = file.write_all(contents.as_bytes()) {
        // Only this call can have created `path`, so only this call removes it.
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

#[cfg(unix)]
pub fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn restrict_dir(_path: &Path) {}

/// Create `path` and any missing parent, owner-only on Unix. The mode is set
/// by the `mkdir` call itself, so no later path lookup can be swapped for a
/// different directory. An existing directory keeps its mode.
///
/// # Errors
/// A component is a file, a component is not a directory, or the caller lacks
/// permission to create the directory.
#[cfg(unix)]
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// Non-Unix platforms have no mode bits, so this creates the directory only.
///
/// # Errors
/// A component is a file, or the caller lacks permission to create the
/// directory.
#[cfg(not(unix))]
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod write_tests;
