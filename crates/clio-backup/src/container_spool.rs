// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the archive writer's member spool: a
//! temporary file that holds one member while its size is being measured.
//!
//! ## Owns
//! - Creating, reading back, and removing the per-member spool file
//!
//! ## Does not own
//! - The ustar header or the payload bytes' meaning (see `tar_header`)
//! - The gzip stream and member ordering (see `container`)
//!
//! ## Boundary
//!
//! Changes to how members are staged during writing should normally
//! require changes here. Changes to archive layout should not require
//! changes here.
//!
//! Keep this module focused on the member spool file.

use super::io_internal;
use clio_types::{AmError, ErrorCode};
use std::io::Read;
use std::path::Path;

/// The open options for a fresh spool file: create-new, read/write, and on a
/// Unix host the owner-only mode.
#[cfg(unix)]
fn spool_options() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true).read(true).mode(0o600);
    options
}

#[cfg(not(unix))]
fn spool_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true).read(true);
    options
}

/// The error when every retry name for the spool file already exists.
const SPOOL_NAME_EXHAUSTED: &str = "could not create a unique temporary spool file for the member";

/// Spool one member to a temporary file while its size is being measured.
///
/// A ustar header stores the member size before the payload, and the gzip
/// stream is not seekable, so the size must be known before anything is
/// written. The spool keeps writer memory bounded: the member streams
/// through a fixed-size chunk buffer, never through a whole-member buffer
/// in memory.
pub(super) struct MemberSpool {
    pub(super) file: std::fs::File,
    path: std::path::PathBuf,
}

impl MemberSpool {
    /// Create the spool file in `dir`. The writer passes the system temp
    /// directory and a nanosecond tag; two spools with the same tag retry
    /// on distinct attempt suffixes.
    ///
    /// The spool carries raw store bytes, so on a multi-user host the file is
    /// created owner-only (`0600`): the same rule the key store and the
    /// deployment config apply to their files.
    pub(super) fn create(dir: &Path, tag: u128) -> Result<Self, AmError> {
        for attempt in 0..8u32 {
            let path = dir.join(format!(
                "clio-backup-member-{}-{tag}-{attempt}.tmp",
                std::process::id()
            ));
            match spool_options().open(&path) {
                Ok(file) => {
                    return Ok(Self { file, path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(io_internal(
                        "creating a temporary spool file for the member",
                        &e,
                    ));
                }
            }
        }
        Err(AmError::new(ErrorCode::Internal, SPOOL_NAME_EXHAUSTED))
    }

    /// Rewind the spool for the write-back pass.
    ///
    /// # Panics
    ///
    /// Rewinding a regular file the writer just created cannot fail; a
    /// failure here means the filesystem broke underneath the writer.
    pub(super) fn rewind(&mut self) {
        use std::io::Seek;
        self.file
            .rewind()
            .expect("rewinding a fresh spool file cannot fail");
    }
}

impl Read for MemberSpool {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Drop for MemberSpool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
