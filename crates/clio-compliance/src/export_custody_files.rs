// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the managed file mechanics of controlled
//! export delivery: staging bytes under a product-controlled name, writing
//! the deterministic same-directory destination temp, sweeping the managed
//! leftovers a crash left, and the small file helpers the delivery sequence
//! uses.
//!
//! ## Owns
//! - The staging write and removal and the destination temp name and write
//! - The managed-leftover sweep reconciliation uses, by exact managed name
//! - The owner-only file open, the directory flush, and the io-error mapping
//!
//! ## Does not own
//! - The delivery sequence and its custody decisions (`export_custody`)
//! - The custody-row and event persistence SQL (`clio-store` owns both)
//!
//! ## Boundary
//!
//! Changes to how or where managed files are named, written, or swept should
//! normally require changes here. Changes to the delivery order should not.
//!
//! Keep this module focused on the managed file mechanics.

use super::DeliveryFailure;
use clio_types::{AmError, ErrorCode};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The product staging directory names the managed sweep covers. The runtime
/// stages subject export bytes in `clio-export-staging` and review exports in
/// `clio-review-staging`.
pub(crate) const STAGING_DIR_NAMES: [&str; 2] = ["clio-export-staging", "clio-review-staging"];

/// The full path of one product staging directory.
pub(crate) fn staging_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

/// The staged-bytes path of one action inside a staging directory.
fn staging_path(root: &Path, action_id: &str) -> PathBuf {
    root.join(format!("{}.staging", sanitize(action_id)))
}

/// The deterministic destination temp of one action: a hidden sibling beside
/// `destination` that reconciliation can find and remove after a crash.
pub(crate) fn destination_temp_path(destination: &Path, action_id: &str) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    destination.with_file_name(format!(".{name}.{}.deliver", sanitize(action_id)))
}

/// Stage `bytes` in `staging_dir` under a product-controlled name.
///
/// A readable leftover product staging file is removed first; a link or a
/// directory at that name is never followed or removed. The file is created
/// with create-new and synced.
pub(crate) fn stage_staging(
    staging_dir: &Path,
    action_id: &str,
    bytes: &[u8],
) -> Result<PathBuf, AmError> {
    fs::create_dir_all(staging_dir).map_err(|err| io_error("create", staging_dir, &err))?;
    let path = staging_path(staging_dir, action_id);
    if let Ok(meta) = fs::symlink_metadata(&path)
        && meta.file_type().is_file()
    {
        fs::remove_file(&path).map_err(|err| io_error("remove", &path, &err))?;
    }
    let mut file = open_owner_only(&path).map_err(|err| io_error("create", &path, &err))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error("write", &path, &err))?;
    Ok(path)
}

/// Remove a managed leftover: a regular file at an exact managed path, never
/// following a link out of its parent.
pub(crate) fn remove_leftover(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_file()
    {
        let _ = fs::remove_file(path);
    }
}

/// Sweep the managed leftovers a crash left for `action_id`: the staged bytes
/// in each product staging directory and the deterministic destination temp
/// beside the recorded destination. Only exact managed names are removed; a
/// destination file itself is never touched.
pub(crate) fn sweep_leftovers(action_id: &str, destination: &Path) {
    for name in STAGING_DIR_NAMES {
        remove_leftover(&staging_path(&staging_root(name), action_id));
    }
    let has_parent = destination
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    if has_parent {
        remove_leftover(&destination_temp_path(destination, action_id));
    }
}

/// Whether a regular file sits at the recorded destination.
pub(crate) fn destination_present(destination: &Path) -> bool {
    matches!(fs::symlink_metadata(destination), Ok(meta) if meta.file_type().is_file())
}

/// Write `bytes` to the deterministic same-directory temp sibling of
/// `destination` for `action_id`.
///
/// A readable leftover temp from a crashed delivery of the same action is
/// removed first; a link or a directory at that name is never followed or
/// removed. The file is created with create-new and synced, so the link in
/// the caller stays on one filesystem and cannot cross a mount.
pub(crate) fn write_destination_temp(
    destination: &Path,
    action_id: &str,
    bytes: &[u8],
) -> Result<PathBuf, DeliveryFailure> {
    let tmp = destination_temp_path(destination, action_id);
    if let Ok(meta) = fs::symlink_metadata(&tmp)
        && meta.file_type().is_file()
    {
        let _ = fs::remove_file(&tmp);
    }
    let mut file = match open_owner_only(&tmp) {
        Ok(file) => file,
        Err(err) => return Err(DeliveryFailure::BeforeWrite(io_error("create", &tmp, &err))),
    };
    if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(DeliveryFailure::BeforeWrite(io_error("write", &tmp, &err)));
    }
    Ok(tmp)
}

/// Open a fresh owner-only file: `0o600` on unix, the default elsewhere.
///
/// The staged and destination-temp files hold export bytes, so they are
/// owner-only from the first byte.
pub(crate) fn open_owner_only(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Flush a directory entry so a link into it survives a crash.
pub(crate) fn fsync_dir(dir: &Path) -> Result<(), AmError> {
    let file = fs::File::open(dir).map_err(|err| io_error("open", dir, &err))?;
    file.sync_all().map_err(|err| io_error("sync", dir, &err))
}

/// A staging-name suffix that cannot escape the staging directory.
fn sanitize(action_id: &str) -> String {
    action_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Map a file error at `path` to the crate error type.
pub(crate) fn io_error(action: &str, path: &Path, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("failed to {action} {}: {err}", path.display()),
    )
}
