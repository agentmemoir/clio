// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the timestamped owner-only backup taken before
//! a deployment config is replaced.
//!
//! ## Owns
//! - The `backups/` sibling directory and its timestamped file names
//! - The owner-only mode of a newly created backup directory, and of the backup
//!   file
//!
//! ## Does not own
//! - Deciding when to back up (the caller does)
//! - The owner-only write primitive itself (see `write`)
//! - The default file name used as a fallback (see `path`)
//!
//! ## Policy
//!
//! The `backups/` directory must be a real directory beside the config file. A
//! symbolic link, or any other non-directory entry, at that name is refused. To
//! keep backups on another filesystem, move the config directory there; the
//! backup does not follow a link out of it.
//!
//! ## Guarantee
//!
//! The directory refusal is a check before the create. The target create is
//! exclusive and owner-only, so an entry already at the target name is never
//! followed or truncated.
//!
//! ## Boundary
//!
//! Changes to backup naming, location, or permissions belong here.
//! Changes to what is backed up should not require changes here.
//!
//! Keep this module focused on the pre-replace backup.

use super::path::DEPLOYMENT_CONFIG_FILE_NAME;
use super::write::{create_new_private, create_private_dir};
use crate::error::{AmError, ErrorCode};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Directory holding timestamped backups of replaced deployment configs.
pub(crate) const BACKUP_DIR_NAME: &str = "backups";

/// Back up `path` into a timestamped `backups/` sibling, creating that
/// directory owner-only when it is missing. Returns the backup path, or `None`
/// when `path` does not exist (nothing to protect).
///
/// A `backups/` directory that already exists keeps the mode it had.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the source is a symbolic link or is not a
/// regular file, when the backups directory is a symbolic link, or when an
/// entry already exists at the backup target name; I/O failures reading the
/// source, inspecting the backups directory, or writing the backup.
pub fn backup_config_file(path: &Path) -> Result<Option<PathBuf>, AmError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    backup_config_file_with_stamp(path, stamp)
}

/// The `backups/` sibling of `path`. A source that carries no parent name still
/// gets a relative `./backups`, never a directory joined onto an empty path.
fn backup_dir_for(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUP_DIR_NAME)
}

/// The backup body with the nanosecond stamp supplied, so a test can predict the
/// target name the write must refuse.
#[allow(clippy::too_many_lines)] // one linear open, inspect, prepare, read, and write
fn backup_config_file_with_stamp(path: &Path, stamp: u128) -> Result<Option<PathBuf>, AmError> {
    // One open: the bytes the backup holds are read from the same descriptor
    // the type check accepted, never from a second lookup by path.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `O_NOFOLLOW` refuses a link at the source instead of backing up what
        // it points at; `O_NONBLOCK` keeps the open from waiting on a FIFO,
        // which the file-type check below then refuses.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "backup source {}: refused because it is a symbolic link; replace it with \
                     the file itself and run again",
                    path.display()
                ),
            ));
        }
        Err(e) => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!("failed to read {} for backup: {e}", path.display()),
            ));
        }
    };
    let meta = file.metadata().map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to read {} for backup: {e}", path.display()),
        )
    })?;
    if !meta.is_file() {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!(
                "backup source {}: refused because it is not a regular file",
                path.display()
            ),
        ));
    }
    let dir = backup_dir_for(path);
    // Inspect the entry before creating it, so a planted link or file is
    // refused with a failure the operator can act on instead of one raised
    // inside the directory create. Only a missing entry is created.
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "backup dir {}: refused because it is a symbolic link; replace it with a \
                     real directory and run again",
                    dir.display()
                ),
            ));
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "failed to create backup dir {}: not a directory",
                    dir.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(&dir).map_err(|e| {
                AmError::new(
                    ErrorCode::Internal,
                    format!("failed to create backup dir {}: {e}", dir.display()),
                )
            })?;
            // A link swapped in during the create is still caught: the entry
            // that now exists must be a real directory.
            let made = fs::symlink_metadata(&dir).map_err(|e| {
                AmError::new(
                    ErrorCode::Internal,
                    format!("failed to inspect backup dir {}: {e}", dir.display()),
                )
            })?;
            if !made.is_dir() {
                return Err(AmError::new(
                    ErrorCode::Forbidden,
                    format!(
                        "backup dir {}: refused because it is a symbolic link or not a \
                         directory; replace it with a real directory and run again",
                        dir.display()
                    ),
                ));
            }
        }
        Err(e) => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!("failed to inspect backup dir {}: {e}", dir.display()),
            ));
        }
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(DEPLOYMENT_CONFIG_FILE_NAME);
    let target = dir.join(format!("{name}.{stamp}"));
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to read {} for backup: {e}", path.display()),
        )
    })?;
    create_new_private(&target, &text).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "backup target {}: refused because an entry with that name already exists; \
                     remove or rename it and run again",
                    target.display()
                ),
            )
        } else {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to write backup {}: {e}", target.display()),
            )
        }
    })?;
    Ok(Some(target))
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod backup_tests;

#[cfg(test)]
#[path = "backup_refusal_tests.rs"]
mod backup_refusal_tests;
