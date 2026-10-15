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
//! - The owner-only permission of the backup directory and file
//!
//! ## Does not own
//! - Deciding when to back up (the caller does)
//! - The owner-only write primitive itself (see `write`)
//! - The default file name used as a fallback (see `path`)
//!
//! ## Boundary
//!
//! Changes to backup naming, location, or permissions belong here.
//! Changes to what is backed up should not require changes here.
//!
//! Keep this module focused on the pre-replace backup.

use super::path::DEPLOYMENT_CONFIG_FILE_NAME;
use super::write::{restrict_dir, write_private};
use crate::error::{AmError, ErrorCode};
use std::fs;
use std::path::{Path, PathBuf};

/// Directory holding timestamped backups of replaced deployment configs.
pub(crate) const BACKUP_DIR_NAME: &str = "backups";

/// Back up `path` into a timestamped `backups/` sibling with owner-only
/// permissions. Returns the backup path, or `None` when `path` does not exist
/// (nothing to protect).
///
/// # Errors
/// I/O failures reading the source or writing the backup.
pub fn backup_config_file(path: &Path) -> Result<Option<PathBuf>, AmError> {
    if !path.exists() {
        return Ok(None);
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUP_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to create backup dir {}: {e}", dir.display()),
        )
    })?;
    restrict_dir(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(DEPLOYMENT_CONFIG_FILE_NAME);
    let target = dir.join(format!("{name}.{stamp}"));
    let text = fs::read_to_string(path).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to read {} for backup: {e}", path.display()),
        )
    })?;
    write_private(&target, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write backup {}: {e}", target.display()),
        )
    })?;
    Ok(Some(target))
}
