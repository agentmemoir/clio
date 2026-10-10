// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for installing the local dev-KMS key-store file:
//! atomic creation, temp-file naming, owner-only permissions, and
//! parent-directory creation.
//!
//! ## Owns
//! - Atomic empty-store installation (hard link, rename fallback)
//! - Temp file names, permissions, and parent directories
//!
//! ## Does not own
//! - The SQLite schema and row operations (see `local_kms_db`)
//! - The v2 stored value shape (see `kms_migrate`)
//! - The DEK provider and its crypto (see `dek`)
//!
//! ## Boundary
//!
//! Changes to how a store is created should normally require changes here.
//! Changes to the stored value shape should require changes in `kms_migrate`
//! and not here. Changes to the SQLite schema or DEK lifecycle should not
//! require changes here.
//!
//! Keep this module focused on store installation.
//!
//! ## Concurrency
//!
//! An empty store is installed with a hard link, so two processes creating it
//! at once never replace each other's open file.

use crate::local_kms_db::{init_schema, seed_format_version};
use crate::migrate::IntoAmError;
use clio_types::AmError;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes temp files created by concurrent threads in one process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Install a fully initialized empty store at `path`.
///
/// The hard link only succeeds when `path` is still absent, so concurrent
/// creators keep one file instead of replacing each other's open store.
///
/// # Errors
/// The store cannot be built, linked, or renamed into place.
pub(crate) fn install_empty_store(path: &Path) -> Result<(), AmError> {
    let tmp = temp_path(path, "create");
    let _ = std::fs::remove_file(&tmp);
    let built = (|| {
        let conn = Connection::open(&tmp).am("create key store")?;
        init_schema(&conn)?;
        seed_format_version(&conn)
    })();
    let built = built.and_then(|()| tighten_permissions(&tmp));
    if let Err(error) = built {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    let installed = match std::fs::hard_link(&tmp, path) {
        Ok(()) => Ok(()),
        // Another process installed the store first; keep theirs.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        // Filesystems without hard links fall back to an atomic rename.
        Err(_) => std::fs::rename(&tmp, path).am("install key store"),
    };
    let _ = std::fs::remove_file(&tmp);
    installed
}

/// Per-process, per-call temp name beside the store, so concurrent creators
/// never share a temp file and the install stays on one filesystem.
pub(crate) fn temp_path(path: &Path, kind: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".{}.{seq}.{kind}", std::process::id()));
    path.with_file_name(name)
}

/// Create the store's parent directory when it is missing.
///
/// # Errors
/// The directory cannot be created.
pub(crate) fn create_parent_dir(path: &Path) -> Result<(), AmError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).am("create key-store directory")?;
    }
    Ok(())
}

/// Tighten the store to owner-only permissions on Unix.
///
/// # Errors
/// The permissions cannot be changed.
#[cfg(unix)]
pub(crate) fn tighten_permissions(path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .am("set key-store permissions")
}

/// Permissions are already process-scoped on non-Unix hosts.
///
/// # Errors
/// Never.
#[cfg(not(unix))]
pub(crate) fn tighten_permissions(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

#[cfg(test)]
#[path = "local_kms_migrate_tests.rs"]
mod tests;
