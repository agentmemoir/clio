// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving one compose-root entry once and doing every
//! later operation on that same held object: the directory walk, the leaf open, the read,
//! the backup create, and the rewrite.
//!
//! ## Owns
//! - `Root`, the held compose root, and the no-follow walk that reaches a parent directory
//! - `HeldFile`, the one leaf descriptor named by its parent and its bare name
//! - The identity check that says the held object is still the one at that name
//! - The sibling backup create, which never overwrites an existing name
//! - The in-place rewrite through the held descriptor, after a truncate
//!
//! ## Does not own
//! - Which assets are materialized and what they contain (see `compose_assets`)
//! - The warning text and the error the caller reports (see `compose_assets`)
//! - Compose-root resolution and clobber protection (see `compose_cli`)
//!
//! ## Boundary
//!
//! Changes to how a compose-root entry is opened, read, backed up, or replaced should
//! normally require changes here. Changes to the materialized set should not require
//! changes here.
//!
//! Why the held form: a path checked then read or written resolves its ancestors again,
//! so a component swapped for a symlink in between redirects the operation. One walk into
//! a descriptor, then a bare name relative to it, keeps every operation on the object the
//! walk reached; the identity check refuses a name that changed under the held descriptor.
//!
//! Keep this module focused on one held file inside the compose root.

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, SeekFrom, Stat, fstat, fsync, ftruncate, mkdirat, open,
    openat, seek, statat,
};
use rustix::io::{Errno, read, write};
use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

/// How many numbered backup names are tried before the create is reported as failed.
const MAX_BACKUP_ATTEMPTS: u32 = 64;

/// The held compose root. Every walk starts here, so no later operation resolves an
/// ancestor of the root again.
pub(crate) struct Root {
    fd: OwnedFd,
    shown: PathBuf,
}

impl Root {
    /// Open `root` once. The path is canonicalized first, so a root reached through a
    /// symlinked ancestor such as macOS `/tmp` still opens, and the descriptor is the one
    /// every later operation names.
    pub(crate) fn open(root: &Path) -> Result<Self, String> {
        let resolved = std::fs::canonicalize(root)
            .map_err(|err| format!("cannot create {}: {err}", root.display()))?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = open(resolved.as_path(), flags, Mode::empty())
            .map_err(|err| format!("cannot create {}: {err}", root.display()))?;
        Ok(Self {
            fd,
            shown: root.to_path_buf(),
        })
    }

    /// Walk `relative_parent` from the held root and hold the directory it reaches. Each
    /// component is opened once, without following a link, so a component swapped after
    /// the walk is not the one a later bare name reaches. A missing component is created
    /// and then opened as one step.
    pub(crate) fn parent(&self, relative_parent: &Path) -> Result<OwnedFd, String> {
        let mut dir = self
            .fd
            .try_clone()
            .map_err(|err| format!("cannot create {}: {err}", self.shown.display()))?;
        let mut walked = self.shown.clone();
        for component in relative_parent.components() {
            match component {
                std::path::Component::Normal(name) => {
                    walked.push(name);
                    dir = step(&dir, name, &walked)?;
                }
                std::path::Component::CurDir => {}
                _ => {
                    return Err(format!(
                        "refuses {}: a compose asset path has only plain components",
                        relative_parent.display()
                    ));
                }
            }
        }
        Ok(dir)
    }

    /// Open the leaf `name` in the held `parent` and keep the descriptor. A new name is
    /// created; an existing one is opened without following a link and without truncating
    /// it. The returned object records the identity of the file that was opened.
    pub(crate) fn hold(parent: OwnedFd, name: &OsStr, shown: &Path) -> Result<HeldFile, String> {
        let create = OFlags::RDWR
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC;
        let (fd, created) = match openat(&parent, name, create, Mode::from(0o644)) {
            Ok(fd) => (fd, true),
            Err(Errno::EXIST) => {
                let open_existing =
                    OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
                let fd = openat(&parent, name, open_existing, Mode::empty())
                    .map_err(|err| format!("cannot write {}: {err}", shown.display()))?;
                (fd, false)
            }
            Err(err) => return Err(format!("cannot write {}: {err}", shown.display())),
        };
        let stat = fstat(&fd).map_err(|err| format!("cannot write {}: {err}", shown.display()))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(format!(
                "refuses {}: it is not a regular file",
                shown.display()
            ));
        }
        if stat.st_nlink != 1 {
            return Err(format!(
                "refuses {}: it has {} hard links, so another name outside the root shares the file",
                shown.display(),
                stat.st_nlink
            ));
        }
        Ok(HeldFile {
            fd,
            parent,
            name: name.to_os_string(),
            stat,
            created,
        })
    }

    /// The type of the entry `relative` names, or `None` when nothing is named there. The
    /// parents are walked without following a link and the leaf is inspected without
    /// following one, so the answer is the entry a later operation would reach.
    pub(crate) fn entry_type(&self, relative: &Path) -> Result<Option<FileType>, String> {
        let (parent_path, name) = split(relative)?;
        let parent = self.parent(parent_path)?;
        match statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
            Err(Errno::NOENT) => Ok(None),
            Err(err) => Err(format!("cannot inspect {}: {err}", relative.display())),
        }
    }
}

/// One open leaf, named by the directory descriptor it was opened from and its bare name.
/// Every operation after the open acts on the descriptor, never on the path again.
pub(crate) struct HeldFile {
    fd: OwnedFd,
    parent: OwnedFd,
    name: OsString,
    stat: Stat,
    created: bool,
}

impl HeldFile {
    /// Whether this descriptor was created by the hold, so it held no earlier content.
    pub(crate) fn was_created(&self) -> bool {
        self.created
    }

    /// Read every byte of the held file from the start.
    pub(crate) fn read_all(&self, shown: &Path) -> Result<Vec<u8>, String> {
        seek(&self.fd, SeekFrom::Start(0))
            .map_err(|err| format!("cannot read {}: {err}", shown.display()))?;
        let mut bytes = Vec::new();
        let mut buf = [0_u8; 8192];
        loop {
            match read(&self.fd, &mut buf[..]) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&buf[..read]),
                Err(Errno::INTR) => {}
                Err(err) => return Err(format!("cannot read {}: {err}", shown.display())),
            }
        }
        Ok(bytes)
    }

    /// Whether the name still resolves to the file this descriptor holds. A name that was
    /// swapped for another file, or removed, is not the object that was read.
    pub(crate) fn still_named(&self, shown: &Path) -> Result<bool, String> {
        match statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(stat.st_dev == self.stat.st_dev && stat.st_ino == self.stat.st_ino),
            Err(Errno::NOENT) => Ok(false),
            Err(err) => Err(format!("cannot inspect {}: {err}", shown.display())),
        }
    }

    /// Create a sibling backup of the held file in the same held parent and write `bytes`
    /// to it. The name has the visible backup shape; an existing name is skipped, never
    /// overwritten, and the next number is tried. Returns the created bare name.
    pub(crate) fn backup(
        &self,
        bytes: &[u8],
        stamp: u64,
        shown: &Path,
    ) -> Result<OsString, String> {
        let base = backup_name(Path::new(&self.name), stamp);
        let Some(base_name) = base.file_name() else {
            return Err(format!("cannot back up {}: no file name", shown.display()));
        };
        let base_name = base_name.to_os_string();
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        for attempt in 0..=MAX_BACKUP_ATTEMPTS {
            let candidate = match attempt {
                0 => base_name.clone(),
                n => OsString::from(format!("{}-{n}", base_name.to_string_lossy())),
            };
            match openat(&self.parent, &candidate, flags, Mode::from(0o600)) {
                Ok(fd) => {
                    write_all(&fd, bytes, "back up", shown)?;
                    fsync(&fd)
                        .map_err(|err| format!("cannot back up {}: {err}", shown.display()))?;
                    return Ok(candidate);
                }
                Err(Errno::EXIST) => {}
                Err(err) => return Err(format!("cannot back up {}: {err}", shown.display())),
            }
        }
        Err(format!(
            "cannot back up {}: no free backup name",
            shown.display()
        ))
    }

    /// Replace every byte of the held file through its descriptor, then report whether the
    /// name still resolves to the file that was written.
    pub(crate) fn rewrite(&self, bytes: &[u8], shown: &Path) -> Result<bool, String> {
        ftruncate(&self.fd, 0).map_err(|err| format!("cannot write {}: {err}", shown.display()))?;
        seek(&self.fd, SeekFrom::Start(0))
            .map_err(|err| format!("cannot write {}: {err}", shown.display()))?;
        write_all(&self.fd, bytes, "write", shown)?;
        fsync(&self.fd).map_err(|err| format!("cannot write {}: {err}", shown.display()))?;
        self.still_named(shown)
    }
}

/// Split a relative compose asset path into its parent components and its leaf name.
pub(crate) fn split(relative: &Path) -> Result<(&Path, &OsStr), String> {
    let Some(name) = relative.file_name() else {
        return Err(format!(
            "{} is not a compose asset file path",
            relative.display()
        ));
    };
    Ok((relative.parent().unwrap_or_else(|| Path::new("")), name))
}

/// The visible backup name for `path` at `stamp`: the extension keeps its bytes, the
/// backup suffix is appended, and the name stays beside the file. Tests call this with a
/// fixed stamp so they never read the wall clock.
pub(crate) fn backup_name(path: &Path, stamp: u64) -> PathBuf {
    path.with_extension(format!(
        "{}.bak-{stamp}",
        path.extension().unwrap_or_default().to_string_lossy()
    ))
}

/// Open one path component relative to the directory already held, creating a missing one
/// first. A symlink or a non-directory is refused; the check and the open name the same
/// entry because the open itself is the check.
fn step(dir: &OwnedFd, name: &OsStr, shown: &Path) -> Result<OwnedFd, String> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match openat(dir, name, flags, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(Errno::NOENT) => {
            match mkdirat(dir, name, Mode::from(0o755)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(err) => return Err(format!("cannot create {}: {err}", shown.display())),
            }
            openat(dir, name, flags, Mode::empty()).map_err(|err| step_error(err, shown))
        }
        Err(err) => Err(step_error(err, shown)),
    }
}

/// The refusal a component open receives, naming a symlink as the reason when it is one.
fn step_error(err: Errno, shown: &Path) -> String {
    match err {
        Errno::LOOP => format!(
            "refuses the symlink at {}: every path component must be a real directory",
            shown.display()
        ),
        Errno::NOTDIR => format!("cannot create {}: it is not a directory", shown.display()),
        _ => format!("cannot create {}: {err}", shown.display()),
    }
}

/// Write every byte to the held descriptor.
fn write_all(fd: &OwnedFd, mut bytes: &[u8], action: &str, shown: &Path) -> Result<(), String> {
    while !bytes.is_empty() {
        match write(fd, bytes) {
            Ok(0) => {
                return Err(format!(
                    "cannot {action} {}: wrote zero bytes",
                    shown.display()
                ));
            }
            Ok(written) => bytes = &bytes[written..],
            Err(Errno::INTR) => {}
            Err(err) => return Err(format!("cannot {action} {}: {err}", shown.display())),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "compose_assets_fs_permissions_tests.rs"]
mod permissions_tests;
#[cfg(test)]
#[path = "compose_assets_fs_tests.rs"]
mod tests;
