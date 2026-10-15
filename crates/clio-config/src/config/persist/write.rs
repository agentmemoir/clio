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
//! - The monotonic counter that keeps concurrent temp files distinct
//! - `restrict_dir`, the owner-only directory rule
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
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter for temp-file names. PID plus nanos is not
/// enough on its own: two writers can read the same clock value, so the
/// counter guarantees distinct temp siblings even in the same nanosecond.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically, owner-only on Unix. The temp file
/// lives beside the target so the final rename stays on one filesystem.
/// Crate-shared: the token store saves through the same writer.
///
/// # Errors
/// The temp file cannot be written, or the final rename fails (the temp file
/// is removed before returning).
pub fn write_private_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    write_private(&tmp, contents)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
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

#[cfg(unix)]
pub fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn restrict_dir(_path: &Path) {}
