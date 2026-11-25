// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for replacing one already-resolved file through a
//! directory descriptor: it judges the final entry, creates a same-directory
//! temp sibling, writes and syncs it, and renames it over the entry.
//!
//! ## Owns
//! - One directory descriptor per write, and the final entry name inside it
//! - Refusing a final entry that is not a regular file
//! - The temp sibling's name, mode, and removal when a step fails
//! - The path-based fallback for a target with no `openat`
//!
//! ## Does not own
//! - Resolving symlinks, reading, or backing up the target; the caller does
//! - Serializing the value that becomes the file's bytes
//! - Deciding which config file is edited, or reporting the result
//!
//! ## Boundary
//!
//! Changes to how one file is judged and replaced atomically should normally
//! require changes here. Changes to which file is edited should not.
//!
//! Keep this module focused on the replacement of one resolved file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::agent_harness_write::{WriteError, file_name, io_error, unix_nanos};

/// The resolved target, held once: the parent directory descriptor and the
/// final entry name inside it on Unix, and the path elsewhere. Every step after
/// `open` uses what was held, so no path is looked up again before the rename.
pub(crate) struct AnchoredTarget {
    /// The resolved path, for error messages and for the non-Unix write.
    path: PathBuf,
    #[cfg(unix)]
    dir: rustix::fd::OwnedFd,
    #[cfg(unix)]
    entry: std::ffi::OsString,
}

impl AnchoredTarget {
    /// Open the parent directory of `target`, which already has its symlinks
    /// resolved, and hold it for the write.
    ///
    /// The parent opens without `O_NOFOLLOW`, so a symlinked directory such as
    /// `/tmp` still works; the held descriptor pins it.
    ///
    /// # Errors
    /// `target` has no file name, or its parent cannot be opened.
    #[cfg(unix)]
    pub(crate) fn open(target: &Path) -> Result<Self, WriteError> {
        use rustix::fs::Mode;

        let entry = target.file_name().ok_or_else(|| WriteError::Io {
            detail: format!(
                "failed to replace {}: it has no file name",
                target.display()
            ),
        })?;
        let parent = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let dir = rustix::fs::open(&parent, dir_flags(), Mode::empty())
            .map_err(|err| io_errno("open", &parent, err))?;
        Ok(Self {
            path: target.to_path_buf(),
            dir,
            entry: entry.to_os_string(),
        })
    }

    /// Hold `target` for a path-based write. Every step looks the path up
    /// again, because this target has no `openat`.
    ///
    /// # Errors
    /// Never; the path is kept for the write.
    #[cfg(not(unix))]
    pub(crate) fn open(target: &Path) -> Result<Self, WriteError> {
        Ok(Self {
            path: target.to_path_buf(),
        })
    }

    /// Replace the held entry with `bytes`, keeping the entry's permission
    /// bits, and leave no temp sibling behind.
    ///
    /// # Errors
    /// The final entry is not a regular file, or a create, write, permission,
    /// sync, or rename step fails.
    #[cfg(unix)]
    pub(crate) fn replace(&self, bytes: &[u8]) -> Result<(), WriteError> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags};

        let mode = match rustix::fs::statat(
            &self.dir,
            self.entry.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode).is_file() => {
                Mode::from_raw_mode(stat.st_mode)
            }
            Ok(_) => {
                return Err(WriteError::Io {
                    detail: format!(
                        "failed to replace {}: the final entry is not a regular file",
                        self.path.display()
                    ),
                });
            }
            Err(rustix::io::Errno::NOENT) => Mode::from_raw_mode(0o644),
            Err(err) => return Err(io_errno("inspect", &self.path, err)),
        };

        let tmp = temp_sibling(&self.path);
        let tmp_name = tmp.file_name().ok_or_else(|| WriteError::Io {
            detail: format!("failed to name a temp file for {}", self.path.display()),
        })?;
        let fd = rustix::fs::openat(
            &self.dir,
            tmp_name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode,
        )
        .map_err(|err| io_errno("create", &tmp, err))?;
        let mut file = std::fs::File::from(fd);
        if let Err(err) = write_and_sync_at(&mut file, bytes, mode, &tmp) {
            remove_temp(&self.dir, tmp_name);
            return Err(err);
        }
        if let Err(err) =
            rustix::fs::renameat(&self.dir, tmp_name, &self.dir, self.entry.as_os_str())
        {
            remove_temp(&self.dir, tmp_name);
            return Err(io_errno("replace", &self.path, err));
        }
        Ok(())
    }

    /// Replace the path with `bytes` through a path-named sibling.
    ///
    /// # Errors
    /// A create, write, permission, sync, or rename step fails.
    #[cfg(not(unix))]
    pub(crate) fn replace(&self, bytes: &[u8]) -> Result<(), WriteError> {
        let path = &self.path;
        let mode = 0o644;
        let (tmp, file) = create_temp(path, mode)?;
        if let Err(err) = write_and_sync(file, bytes, mode, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io_error("replace", path, &e));
        }
        Ok(())
    }
}

/// The flags to open a directory that only serves as an `*at()` handle.
///
/// `O_PATH` needs no read permission on the directory, and the kernel still
/// accepts it as the `dirfd` of `statat`, `openat`, `renameat`, and `unlinkat`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn dir_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC
}

/// The flags to open a directory that only serves as an `*at()` handle.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn dir_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC
}

/// A hidden temp sibling beside `path`, unique across threads and processes.
fn temp_sibling(path: &Path) -> PathBuf {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let name = file_name(path);
    let pid = std::process::id();
    let nanos = unix_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{pid}.{nanos}.{seq}.tmp"))
}

/// Write `bytes` to the temp `file`, restore the exact mode in case the
/// process mask narrowed it at creation, and sync to disk before the caller
/// renames the temp file over its target.
#[cfg(unix)]
fn write_and_sync_at(
    file: &mut std::fs::File,
    bytes: &[u8],
    mode: rustix::fs::Mode,
    tmp: &Path,
) -> Result<(), WriteError> {
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|e| io_error("write", tmp, &e))?;
    rustix::fs::fchmod(&*file, mode).map_err(|err| io_errno("set permissions on", tmp, err))?;
    file.sync_all().map_err(|e| io_error("sync", tmp, &e))?;
    Ok(())
}

/// Remove the temp sibling, tolerating its absence: the caller already holds
/// the write error, and a removal failure cannot replace it.
#[cfg(unix)]
fn remove_temp(dir: &rustix::fd::OwnedFd, name: &std::ffi::OsStr) {
    let _ = rustix::fs::unlinkat(dir, name, rustix::fs::AtFlags::empty());
}

#[cfg(unix)]
fn io_errno(action: &str, path: &Path, err: rustix::io::Errno) -> WriteError {
    WriteError::Io {
        detail: format!("failed to {action} {}: {err}", path.display()),
    }
}

/// Create a new temp sibling of `path` with `mode` already applied, so the
/// file never exists with wider permissions. The name is unique across threads
/// and processes; a leftover name from a crashed run is skipped.
#[cfg(not(unix))]
fn create_temp(path: &Path, mode: u32) -> Result<(PathBuf, std::fs::File), WriteError> {
    for _ in 0..10 {
        let tmp = temp_sibling(path);
        match open_new(&tmp, mode) {
            Ok(file) => return Ok((tmp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error("create", &tmp, &err)),
        }
    }
    Err(WriteError::Io {
        detail: format!("failed to create a temp file beside {}", path.display()),
    })
}

#[cfg(not(unix))]
fn open_new(tmp: &Path, _mode: u32) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
}

/// Write `bytes` to the temp `file` and sync to disk before the caller renames
/// the temp file over its target.
#[cfg(not(unix))]
fn write_and_sync(
    file: std::fs::File,
    bytes: &[u8],
    mode: u32,
    tmp: &Path,
) -> Result<(), WriteError> {
    use std::io::Write;
    let mut file = file;
    file.write_all(bytes)
        .map_err(|e| io_error("write", tmp, &e))?;
    set_mode(tmp, mode).map_err(|e| io_error("set permissions on", tmp, &e))?;
    file.sync_all().map_err(|e| io_error("sync", tmp, &e))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "agent_harness_atomic_write_tests.rs"]
mod tests;
