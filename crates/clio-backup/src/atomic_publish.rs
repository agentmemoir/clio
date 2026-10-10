// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the atomic publish: the file that takes its
//! final name only once it is complete, and the directory flush that makes the
//! rename survive a crash.
//!
//! ## Owns
//! - `AtomicFile`: reserve, write, flush, rename, and remove on failure
//! - `sync_dir`: flushing a directory so a rename into it is durable
//!
//! ## Does not own
//! - Staging a member and digesting it (see `staging`)
//! - The archive's contents or member order (see `container`)
//!
//! ## Boundary
//!
//! Changes to how a finished file is published should normally require changes
//! here. Changes to what goes into the file should not require changes here.
//!
//! Keep this module focused on the publish discipline.

use crate::staging::{create_owner_only, io_error, reserve_path};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// A file that becomes its final name only when it is complete.
///
/// The temporary file is created in the destination directory — the same
/// filesystem as the final path, so the rename cannot cross one — with the
/// owner-only mode the archive requires. Until [`AtomicFile::commit`] runs, a
/// directory listing shows a `.tmp` file and no archive; a crash leaves a
/// temporary file, never a partial archive.
#[derive(Debug)]
pub struct AtomicFile {
    temp: PathBuf,
    final_path: PathBuf,
    dir: PathBuf,
    committed: bool,
}

impl AtomicFile {
    /// Reserve the temporary name for `final_path` in its own directory.
    ///
    /// `tag` names the temporary file, so the caller supplies a name that is
    /// already a safe single path component.
    ///
    /// # Errors
    /// The parent directory is unusable.
    pub fn reserve(final_path: PathBuf, tag: &str) -> Result<Self, AmError> {
        // A bare file name has an empty parent, which is not a directory the
        // temporary file could live in.
        let dir = final_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "the archive path {} has no directory to stage in",
                        final_path.display()
                    ),
                )
            })?
            .to_path_buf();
        let temp = reserve_path(&dir, tag)?;
        Ok(Self {
            temp,
            dir,
            final_path,
            committed: false,
        })
    }

    /// The temporary path, before the rename.
    #[must_use]
    pub fn temp_path(&self) -> &Path {
        &self.temp
    }

    /// Create the temporary file owner-only and hand it to the writer.
    ///
    /// # Errors
    /// The temporary file cannot be created.
    pub fn take_writer(&mut self) -> Result<std::fs::File, AmError> {
        create_owner_only(&self.temp)
    }

    /// Flush the finished file to disk and publish it under its final name.
    ///
    /// Flushing the file is not enough on its own. The rename is a change to the
    /// *directory*, and a directory entry only survives a crash once the
    /// directory itself has been flushed, so the destination directory is synced
    /// after the rename. Without that step a power cut can lose an archive that
    /// `create` already reported as written, which is the one outcome a backup
    /// must not produce.
    ///
    /// # Errors
    /// The file cannot be flushed, the rename fails, or the directory cannot be
    /// flushed. The first two leave the temporary file removed by `Drop` and no
    /// archive behind. The third happens after the rename succeeded, so the
    /// archive is present and the error says its durability is unconfirmed.
    pub fn commit(&mut self, file: std::fs::File) -> Result<PathBuf, AmError> {
        if let Err(error) = file.sync_all() {
            return Err(io_error("flushing the archive to disk", &error));
        }
        drop(file);
        std::fs::rename(&self.temp, &self.final_path)
            .map_err(|e| io_error("publishing the archive", &e))?;
        // The rename landed, so the temporary path is gone and there is nothing
        // left for `Drop` to remove. Mark the file committed before the
        // directory sync so a failure there cannot make `Drop` reach for a path
        // that no longer exists.
        self.committed = true;
        if let Err(error) = sync_dir(&self.dir) {
            return Err(io_error(
                &format!(
                    "the archive at {} was published but its directory could not be flushed, so \
                     it may not survive a crash",
                    self.final_path.display()
                ),
                &error,
            ));
        }
        Ok(self.final_path.clone())
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// Flush a directory's own entries to disk, so a rename into it is durable.
///
/// Opening a directory for reading and syncing the handle is the Unix way to
/// flush its entries. The directory is the one the temporary file was reserved
/// in, so the rename that just succeeded landed there. Windows offers no
/// directory handle to sync, so the rename stands on its own there.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "atomic_publish_tests.rs"]
mod tests;
