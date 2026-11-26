// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for finding a harness binary on the process
//! `PATH` and holding the exact file that was inspected until the vendor
//! command starts from it.
//!
//! ## Owns
//! - `lookup_on_path`: the first `PATH` candidate that is a runnable file
//! - `is_runnable`: the file-type and exec-bit test a candidate must pass
//! - `Held`: the opened file, or the canonical executable path on a platform
//!   that cannot start a program from a descriptor
//! - `dir_is_safe`: the ownership and write-bit test for the directories above
//!   the executable on those platforms
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

use std::path::{Path, PathBuf};
use std::process::Command;

/// Find a binary on the process `PATH`: a file the user could run.
pub(crate) fn lookup_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_runnable(candidate))
}

#[cfg(unix)]
pub(crate) fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(crate) fn is_runnable(path: &Path) -> bool {
    path.is_file()
}

/// The harness binary, opened once and named by that descriptor at every spawn.
#[cfg(target_os = "linux")]
pub(crate) struct Held {
    file: std::fs::File,
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
        Ok(Held { file })
    }

    /// A command that starts the held file, with `argv0` as the name it sees.
    pub(crate) fn command(&self, argv0: &Path) -> Command {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(format!("/proc/self/fd/{}", self.file.as_raw_fd()));
        command.arg0(argv0);
        command
    }
}

/// The harness binary where no descriptor can be started: its canonical path,
/// held only after that file and every directory above it is judged safe from
/// another user's replacement.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) struct Held {
    exec_path: PathBuf,
}

#[cfg(all(unix, not(target_os = "linux")))]
impl Held {
    /// Canonicalize `path`, judge the canonical file, then judge its parent
    /// directory chain. A directory another user can write to could swap the
    /// executable after this call, so it is refused.
    ///
    /// # Errors
    /// `path` cannot be resolved, the target is not a regular file or has no
    /// exec bit, or a directory above it is owned by another user or writable
    /// by group or other.
    pub(crate) fn open(path: &Path) -> std::io::Result<Held> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let exec_path = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&exec_path)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        let euid = rustix::process::geteuid().as_raw();
        let mut parent = exec_path.parent();
        while let Some(dir) = parent {
            let meta = std::fs::metadata(dir)?;
            if !meta.is_dir() || !dir_is_safe(meta.uid(), meta.permissions().mode(), euid) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            parent = dir.parent();
        }
        Ok(Held { exec_path })
    }

    /// A command that starts the held path, with `argv0` as the name it sees.
    pub(crate) fn command(&self, argv0: &Path) -> Command {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(&self.exec_path);
        command.arg0(argv0);
        command
    }
}

/// Whether a directory owned by `uid` with `mode` keeps another user from
/// replacing its entries, given the effective user id `euid`.
#[cfg(all(unix, any(test, not(target_os = "linux"))))]
fn dir_is_safe(uid: u32, mode: u32, euid: u32) -> bool {
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

    /// A command that starts the held path, as the earlier path-based spawn
    /// did.
    pub(crate) fn command(&self, _argv0: &Path) -> Command {
        Command::new(&self.path)
    }
}

#[cfg(test)]
#[path = "agent_harness_delegate_path_tests.rs"]
mod path_tests;
