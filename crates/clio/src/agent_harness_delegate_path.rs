// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for listing the `PATH` candidates for a harness
//! binary and holding the exact file that was inspected until the vendor
//! command starts from it.
//!
//! ## Owns
//! - `path_candidates`: the `PATH` entries joined with the binary name, in
//!   search order, with no filesystem check
//! - `Held`: the opened file, or the canonical executable path on a platform
//!   that cannot start a program from a descriptor
//! - `dir_is_safe` and `file_is_safe`: the ownership and write-bit tests the
//!   no-descriptor platform applies to the target and its parent directories
//!
//! ## Does not own
//! - The vendor argument list, its stdin, output capture, and the retry on a
//!   busy executable (see `agent_harness_delegate`)
//! - The harness registry, or which harness is installed (see `agent_harness`)
//! - Reading or writing the harness config file
//!
//! ## Boundary
//!
//! Changes to how a harness binary is located, judged, or held for exec should
//! normally require changes here. Changes to how the vendor command is built,
//! or to what its outcome means, should not.
//!
//! Keep this module focused on the binary between the lookup and the spawn.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `PATH` candidates for `binary`, in search order. No filesystem check:
/// a candidate is judged only when it is opened.
pub(crate) fn path_candidates(binary: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| path_candidates_from(&value, binary))
        .unwrap_or_default()
}

/// [`path_candidates`] for an injected `PATH` value, so a test can name the
/// entries without changing the process environment.
fn path_candidates_from(value: &OsStr, binary: &str) -> Vec<PathBuf> {
    std::env::split_paths(value)
        .map(|dir| dir.join(binary))
        .collect()
}

/// The harness binary, opened once and named by that descriptor at every spawn.
///
/// Requires procfs at `/proc/self/fd`: that is the only name the kernel can
/// start a held descriptor by.
#[cfg(target_os = "linux")]
pub(crate) struct Held {
    file: std::fs::File,
    path: PathBuf,
}

#[cfg(target_os = "linux")]
impl Held {
    /// Open `path` and keep the descriptor. The descriptor is the object
    /// `fstat` judges here and the object the kernel starts, so a name replaced
    /// after this call cannot change what runs.
    ///
    /// `O_PATH` alone: refusing a symbolic link would break package-manager
    /// installs, and close-on-exec would stop a `#!` interpreter from reopening
    /// the descriptor path after the kernel hands it the script.
    ///
    /// # Errors
    /// `path` cannot be opened, or the opened file is not a regular file or has
    /// no exec bit.
    pub(crate) fn open(path: &Path) -> std::io::Result<Held> {
        use rustix::fs::{Mode, OFlags};
        use std::os::unix::fs::PermissionsExt;

        let fd =
            rustix::fs::open(path, OFlags::PATH, Mode::empty()).map_err(std::io::Error::from)?;
        let file = std::fs::File::from(fd);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        Ok(Held {
            file,
            path: path.to_path_buf(),
        })
    }

    /// The candidate path this file was opened from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A command that starts the held file, with its stored path as `argv0`.
    pub(crate) fn command(&self) -> Command {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(format!("/proc/self/fd/{}", self.file.as_raw_fd()));
        command.arg0(self.path());
        command
    }
}

/// The harness binary where no descriptor can be started: its canonical path,
/// held only after that file and every directory above it is judged safe from
/// another user's replacement.
///
/// Group write is accepted on a directory the caller owns, as Homebrew makes
/// its managed directories. A member of such a group can still replace the
/// binary. ACLs are out of scope.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) struct Held {
    path: PathBuf,
    exec_path: PathBuf,
}

#[cfg(all(unix, not(target_os = "linux")))]
impl Held {
    /// Canonicalize `path`, judge the canonical file, then judge its parent
    /// directory chain. A directory another user can write to, or another
    /// user's file, could swap the executable after this call, so it is
    /// refused.
    ///
    /// # Errors
    /// `path` cannot be resolved, the target is not a regular file or has no
    /// exec bit or may be rewritten by another user, or a directory above it
    /// fails the ownership and write-bit test.
    pub(crate) fn open(path: &Path) -> std::io::Result<Held> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let exec_path = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&exec_path)?;
        let euid = rustix::process::geteuid().as_raw();
        let mode = metadata.permissions().mode();
        if !metadata.is_file() || mode & 0o111 == 0 || !file_is_safe(metadata.uid(), mode, euid) {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        let mut parent = exec_path.parent();
        while let Some(dir) = parent {
            let meta = std::fs::metadata(dir)?;
            if !meta.is_dir() || !dir_is_safe(meta.uid(), meta.permissions().mode(), euid) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            parent = dir.parent();
        }
        Ok(Held {
            path: path.to_path_buf(),
            exec_path,
        })
    }

    /// The candidate path this file was opened from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A command that starts the canonical path, with the candidate path as
    /// `argv0`.
    pub(crate) fn command(&self) -> Command {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(&self.exec_path);
        command.arg0(self.path());
        command
    }
}

/// Whether a directory owned by `uid` with `mode` keeps another user from
/// replacing its entries, given the effective user id `euid`. The caller may
/// own it, or root may own it with no group write; other write is never safe.
#[cfg(all(unix, any(test, not(target_os = "linux"))))]
fn dir_is_safe(uid: u32, mode: u32, euid: u32) -> bool {
    (uid == euid || (uid == 0 && (mode & 0o020) == 0)) && (mode & 0o002) == 0
}

/// Whether a file owned by `uid` with `mode` keeps another user from rewriting
/// it, given the effective user id `euid`. Root or the caller must own it, and
/// group and other write must be clear.
#[cfg(all(unix, any(test, not(target_os = "linux"))))]
fn file_is_safe(uid: u32, mode: u32, euid: u32) -> bool {
    (uid == 0 || uid == euid) && (mode & 0o022) == 0
}

/// The harness binary where no descriptor exists: the looked-up path itself.
#[cfg(not(unix))]
pub(crate) struct Held {
    path: PathBuf,
}

#[cfg(not(unix))]
impl Held {
    /// Keep `path` for the spawn; this platform has no descriptor to hold.
    ///
    /// # Errors
    /// Never; the path was already looked up.
    pub(crate) fn open(path: &Path) -> std::io::Result<Held> {
        Ok(Held {
            path: path.to_path_buf(),
        })
    }

    /// The candidate path this file was opened from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A command that starts the held path, as the earlier path-based spawn
    /// did.
    pub(crate) fn command(&self) -> Command {
        Command::new(self.path())
    }
}

#[cfg(test)]
#[path = "agent_harness_delegate_path_tests.rs"]
mod path_tests;
