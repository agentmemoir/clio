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
//! - The unpredictable per-run staging-directory naming and the owner-only
//!   staging-directory creation with its fail-closed reuse rule
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
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::BuildHasher;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The product staging directory names the managed sweep covers. The runtime
/// stages subject export bytes in `clio-export-staging` and review exports in
/// `clio-review-staging`.
pub(crate) const STAGING_DIR_NAMES: [&str; 2] = ["clio-export-staging", "clio-review-staging"];

/// The staging directory one delivery uses: an unpredictable unique name next
/// to the product staging area under the temp root. Another local user can
/// neither guess the name nor plant anything at it before delivery creates it
/// owner-only through [`stage_staging`].
#[must_use]
pub fn unique_staging_dir(product: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{product}-{}", random_suffix()))
}

/// A 128-bit random suffix: two independent hash seeds the standard library
/// draws from the operating system at process start, mixed with the process
/// id, the clock, and a call counter, so no two calls in any process share a
/// name.
fn random_suffix() -> String {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let calls = CALLS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    let first = RandomState::new().hash_one((std::process::id(), nanos, calls));
    let second = RandomState::new().hash_one((calls, nanos, std::process::id()));
    format!("{first:016x}{second:016x}")
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
    ensure_staging_dir(staging_dir)?;
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

/// Create `dir` owner-only when absent, or accept an existing one only when it
/// is a plain directory owned by the current user with 0o700 permissions on
/// unix. Anything else at that path — a link, another user's directory, a
/// wider mode — fails closed and never becomes a staging directory.
fn ensure_staging_dir(dir: &Path) -> Result<(), AmError> {
    match fs::symlink_metadata(dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => create_owner_only(dir),
        Err(err) => Err(io_error("inspect", dir, &err)),
        Ok(meta) if is_plain_owned_dir(&meta) && is_owner_only_mode(&meta) => Ok(()),
        Ok(_) => Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "refusing staging directory {}: it exists but is not a plain directory owned by \
                 the current user with owner-only permissions",
                dir.display()
            ),
        )),
    }
}

/// Whether a directory entry is a plain directory owned by the current user;
/// a link never passes, so a managed removal never follows one.
fn is_plain_owned_dir(meta: &fs::Metadata) -> bool {
    if !meta.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if current_uid().is_none_or(|uid| meta.uid() != uid) {
            return false;
        }
    }
    true
}

/// Whether the directory mode is owner-only 0o700 on unix.
#[cfg(unix)]
fn is_owner_only_mode(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777 == 0o700
}

#[cfg(not(unix))]
fn is_owner_only_mode(_meta: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let probe = std::env::temp_dir().join(format!(".clio-owner-probe-{}", random_suffix()));
    let uid = match open_owner_only(&probe) {
        Ok(file) => file.metadata().ok().map(|meta| meta.uid()),
        Err(_) => None,
    };
    let _ = fs::remove_file(&probe);
    uid
}

/// Create a directory with owner-only 0o700 permissions on unix. The owner
/// bits are the only bits requested, so no umask can widen the result.
#[cfg(unix)]
fn create_owner_only(dir: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(dir)
        .map_err(|err| io_error("create", dir, &err))
}

#[cfg(not(unix))]
fn create_owner_only(dir: &Path) -> Result<(), AmError> {
    fs::DirBuilder::new()
        .create(dir)
        .map_err(|err| io_error("create", dir, &err))
}

/// Remove a delivered action's staged file, and drop the staging directory
/// itself when that removal left it empty. A directory that still holds
/// another action's files fails the removal silently and stays.
pub(crate) fn remove_staged(staging: &Path) {
    remove_leftover(staging);
    if let Some(dir) = staging.parent() {
        remove_dir_if_empty(dir);
    }
}

/// Remove a staging directory when it is a plain directory owned by the
/// current user and empty; anything else fails silently and stays.
fn remove_dir_if_empty(dir: &Path) {
    if let Ok(meta) = fs::symlink_metadata(dir)
        && is_plain_owned_dir(&meta)
    {
        let _ = fs::remove_dir(dir);
    }
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
/// in each product staging directory under the temp root and the deterministic
/// destination temp beside the recorded destination. Staging directories are
/// per-run unpredictable names next to the legacy fixed names, so the sweep
/// covers both. Only exact managed names are removed, and only inside plain
/// directories owned by the current user; a destination file itself is never
/// touched.
pub(crate) fn sweep_leftovers(action_id: &str, destination: &Path) {
    sweep_staging_leftovers(&std::env::temp_dir(), action_id);
    let has_parent = destination
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    if has_parent {
        remove_leftover(&destination_temp_path(destination, action_id));
    }
}

/// The staging part of the sweep over one temp root, so tests can point it at
/// a scratch base instead of the shared system temp root.
pub(crate) fn sweep_staging_leftovers(base: &Path, action_id: &str) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    let staged_name = format!("{}.staging", sanitize(action_id));
    for entry in entries.flatten() {
        let raw_name = entry.file_name();
        let Some(name) = raw_name.to_str() else {
            continue;
        };
        if !STAGING_DIR_NAMES.iter().any(|product| {
            name == *product
                || (name.len() > product.len()
                    && name.starts_with(product)
                    && name.as_bytes()[product.len()] == b'-')
        }) {
            continue;
        }
        let dir = entry.path();
        if let Ok(meta) = fs::symlink_metadata(&dir)
            && is_plain_owned_dir(&meta)
        {
            remove_leftover(&dir.join(&staged_name));
            remove_dir_if_empty(&dir);
        }
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

#[cfg(test)]
#[path = "export_custody_files_tests.rs"]
mod export_custody_files_tests;
