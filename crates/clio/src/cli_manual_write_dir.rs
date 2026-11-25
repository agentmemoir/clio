// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the manual's output directory open and doing every
//! per-page operation relative to it: the walk resolving `--out` once, the target check, the
//! temp write, and the promote.
//!
//! ## Owns
//! - `OutDir`, the held output directory and the one place `--out` is resolved
//! - The component walk: a missing directory is created, a symlink or non-directory is refused
//! - The per-page target check, the same-directory temp, and the promote, as bare names
//! - The plain-name guard keeping a page name to one file in that directory
//! - The refusal and failure messages every operation here shares
//!
//! ## Does not own
//! - The manual's render, write order, and incomplete-manual error shape (see `cli_manual_write`)
//! - The `--markdown`/`--out` argument validation (see `cli_top_level`)
//!
//! ## Boundary
//!
//! Changes to how one page reaches the output directory should normally require changes here; and
//! changes to which pages the manual writes should not. Why the held directory: a path checked then
//! written through resolves its ancestors twice, so a component swapped for a symlink in between
//! redirects the write; one walk into a descriptor, then bare names, keep the write in the directory
//! the walk reached.
//!
//! What that guarantees, and no more: every component of `--out` is opened once, relative to the descriptor held for the component before it; a `..` component is refused rather than opened, because it re-resolves against the held directory's current parent rather than the parent the walk checked; and no operation after the walk resolves a path at all, since the target check, the temp create, the chmod, the sync, and the promote name a bare file inside the held directory.
//!
//! The held-directory form is the only form: the crate declares a descriptor-relative call
//! unconditionally, and every target it ships is one that has them.

use crate::cli_error::CliError;
use crate::cli_manual_write::GENERATED_MARKER;
use crate::cli_manual_write::incomplete;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::{
    fs::{
        AtFlags, FileType, Mode, OFlags, fchmod, mkdirat, open, openat, renameat, statat, unlinkat,
    },
    io::Errno,
};
use std::os::fd::OwnedFd;

/// The target whose promote step fails while the test seam is armed. Test-only.
#[cfg(test)]
pub(crate) static FAILED_PROMOTE_TARGET: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);

/// The `--out` a test seam is armed for, the place the checked directory is
/// moved to, and the decoy directory that takes its name: the seam swapping a
/// component between the walk and the promote. Test-only, and keyed by the armed
/// `--out` so another test's write cannot consume it.
#[cfg(test)]
static SWAP_BEFORE_PROMOTE: std::sync::Mutex<Option<(PathBuf, PathBuf, PathBuf)>> =
    std::sync::Mutex::new(None);

/// The `--out` a test seam is armed for, the page name a symlink is planted at,
/// and the file outside `--out` it points at: the seam planting a target
/// between the check and the write. Test-only, and keyed by the armed `--out` so
/// another test's write cannot consume it.
#[cfg(test)]
static PLANT_AFTER_CHECK: std::sync::Mutex<Option<(PathBuf, PathBuf, OsString)>> =
    std::sync::Mutex::new(None);

/// The manual's output directory, resolved once and held. `shown` is `--out` as the caller
/// gave it, so every message keeps its bytes.
pub(crate) struct OutDir {
    fd: OwnedFd,
    shown: PathBuf,
}

impl OutDir {
    /// Resolve `out_dir` once, creating a missing component as it is reached. Each component is
    /// visited once: an existing one is classified without following a link, a missing one is created
    /// and then opened, and what comes back is a descriptor the later operations name rather than
    /// resolving the path again. A `..` component is refused before anything is created.
    pub(crate) fn open(out_dir: &Path) -> Result<Self, CliError> {
        if out_dir.components().next().is_none() {
            return Err(CliError::usage("--out is empty"));
        }
        // A relative `--out` starts at the working directory; an absolute one at its root.
        let (start, mut walked) = if out_dir.is_absolute() {
            (Path::new("/"), PathBuf::from("/"))
        } else {
            (Path::new("."), PathBuf::new())
        };
        // A `..` is refused before anything is opened or created: the kernel resolves it against the
        // held directory's current parent, which a rename can change after the walk checked it.
        refuse_parent_component(out_dir, &walked)?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let mut fd = open(start, flags, Mode::empty())
            .map_err(|err| io_error("create", start, &err.into()))?;
        for component in out_dir.components() {
            match component {
                std::path::Component::Normal(name) => {
                    fd = step(&fd, &walked, name)?;
                    walked.push(name);
                }
                // Kept in the displayed path, so a refusal repeats the bytes the caller wrote.
                std::path::Component::CurDir => walked.push("."),
                // A `..` is refused above; the root of an absolute path is already open, and a
                // prefix is not a name here, so neither is a step.
                _ => {}
            }
        }
        Ok(Self {
            fd,
            shown: out_dir.to_path_buf(),
        })
    }

    /// Check one page target: absent, generated by this command, or refused. A symlink, a directory,
    /// any other non-regular file, or a file this command did not generate is refused, not replaced.
    pub(crate) fn check_target(&self, file: &str) -> Result<(), CliError> {
        page_file_name(file)?;
        let target = self.shown.join(file);
        let stat = match statat(&self.fd, file, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => return Ok(()),
            Ok(stat) => stat,
            Err(err) => return Err(errno_error("inspect", &target, err)),
        };
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => {}
            FileType::Symlink => return Err(symlink_target(&target)),
            FileType::Directory => return Err(directory_target(&target)),
            _ => return Err(special_target(&target)),
        }
        // The open names the entry classified above; `NONBLOCK` makes a FIFO swapped in after
        // the classification an error rather than a wait. The open is part of reading the marker,
        // so its failure reports the read, as a plain path read of the same file would.
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let opened = openat(&self.fd, file, flags, Mode::empty())
            .map_err(|err| errno_error("read", &target, err))?;
        if !generated(std::fs::File::from(opened), &target)? {
            return Err(unmarked_target(&target));
        }
        Ok(())
    }

    /// Write `bytes` to a fresh temp sibling of `file` in the held directory and return its bare name.
    /// The temp takes a replaced page's mode, or `0o644` for a new page or for anything that is not a
    /// regular file, and is created owner-only so it never exists wider. A target swapped for a symlink
    /// after its check is never followed: the create requires a new name. A failure once the temp exists
    /// removes it and reports the manual incomplete. A test-only seam can plant a target at the page
    /// name before the create.
    pub(crate) fn write_temp(&self, file: &str, bytes: &[u8]) -> Result<OsString, CliError> {
        armed_plant_after_check(&self.shown);
        // A regular file's mode is kept; a link, a fifo, or anything else planted after the check
        // leaves the new page at the default, so an attacker cannot choose a wider one.
        let mode = match statat(&self.fd, file, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => {
                Mode::from_raw_mode(stat.st_mode)
            }
            _ => Mode::from(0o644),
        };
        let (tmp, mut staged) = self.create_temp(file)?;
        let shown = self.shown.join(&tmp);
        if let Err(err) = write_and_sync(&mut staged, bytes, mode, &shown) {
            self.remove_temp(&tmp);
            return Err(incomplete(&self.shown, &err));
        }
        Ok(tmp)
    }

    /// Open a fresh temp sibling of `file`; a leftover name from a crashed run is skipped.
    fn create_temp(&self, file: &str) -> Result<(OsString, std::fs::File), CliError> {
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        for _ in 0..10 {
            let tmp = temp_name(file);
            match openat(&self.fd, tmp.as_os_str(), flags, Mode::RUSR | Mode::WUSR) {
                Ok(opened) => return Ok((tmp, std::fs::File::from(opened))),
                Err(Errno::EXIST) => {}
                Err(err) => return Err(errno_error("create", &self.shown.join(&tmp), err)),
            }
        }
        Err(temp_exhausted(&self.shown.join(file)))
    }

    /// Remove a temp sibling this module created, best effort.
    pub(crate) fn remove_temp(&self, tmp: &OsStr) {
        let _ = unlinkat(&self.fd, tmp, AtFlags::empty());
    }

    /// Rename the temp sibling over its target: the promote step. Both names are bare and sit in the
    /// held directory, so the rename cannot leave it or be redirected by a component that changed
    /// after the walk. Test-only seams can swap the directory at its name, or make this one step
    /// fail for one exact target.
    pub(crate) fn promote(&self, tmp: &OsStr, file: &str) -> Result<(), CliError> {
        armed_swap_before_promote(&self.shown);
        let target = self.shown.join(file);
        if let Some(err) = armed_promote_failure(&target) {
            return Err(err);
        }
        renameat(&self.fd, tmp, &self.fd, file).map_err(|err| errno_error("promote", &target, err))
    }
}

/// Open one path component relative to the directory already held. The classification and the open
/// name the same entry, so the component checked is the one opened; a missing one is created then
/// opened, making the create and the check one lookup. The create uses `0o777` and leaves the mode to
/// the process umask, as a directory create outside this module would.
fn step(dir: &OwnedFd, walked: &Path, name: &OsStr) -> Result<OwnedFd, CliError> {
    let at = walked.join(name);
    match statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {}
            FileType::Symlink => return Err(symlink_component(&at)),
            _ => return Err(not_a_directory(&at)),
        },
        Err(Errno::NOENT) => {
            if let Err(err) = mkdirat(dir, name, Mode::from(0o777))
                && err != Errno::EXIST
            {
                return Err(errno_error("create", &at, err));
            }
        }
        Err(err) => return Err(errno_error("inspect", &at, err)),
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    openat(dir, name, flags, Mode::empty()).map_err(|err| match err {
        Errno::LOOP => symlink_component(&at),
        Errno::NOTDIR => not_a_directory(&at),
        _ => errno_error("create", &at, err),
    })
}

/// The refusal an `--out` containing `..` receives before anything is opened or created: the kernel
/// resolves `..` against the held directory's current parent, which a rename can change after the
/// walk checked it, so one lookup cannot both check and use it.
fn parent_component(walked: &Path) -> CliError {
    let shown = walked.join("..").display().to_string();
    CliError::usage(format!(
        "--out refuses `{shown}`: `..` can resolve to a different directory than the one checked"
    ))
}

/// Refuse an `out_dir` whose walk reaches a `..`, naming the walked path up to it.
fn refuse_parent_component(out_dir: &Path, start: &Path) -> Result<(), CliError> {
    let mut walked = start.to_path_buf();
    for component in out_dir.components() {
        match component {
            std::path::Component::Normal(name) => walked.push(name),
            std::path::Component::CurDir => walked.push("."),
            std::path::Component::ParentDir => return Err(parent_component(&walked)),
            _ => {}
        }
    }
    Ok(())
}

/// The refusal a symlink component receives.
fn symlink_component(at: &Path) -> CliError {
    let shown = at.display();
    CliError::usage(format!(
        "--out refuses the symlink at `{shown}`; every path component must be a real directory"
    ))
}

/// The refusal a component that is not a directory receives.
fn not_a_directory(at: &Path) -> CliError {
    let shown = at.display();
    CliError::usage(format!("--out `{shown}` is not a directory"))
}

/// The refusal a target that is a symlink receives.
fn symlink_target(t: &Path) -> CliError {
    let shown = t.display();
    CliError::usage(format!("refuses to replace the symlink at `{shown}`"))
}

/// The refusal a directory in a page's place receives.
fn directory_target(t: &Path) -> CliError {
    let shown = t.display();
    CliError::usage(format!(
        "refuses `{shown}`: a directory sits in a page's place"
    ))
}

/// The refusal a special file in a page's place receives.
fn special_target(t: &Path) -> CliError {
    let shown = t.display();
    CliError::usage(format!("refuses `{shown}`: it is not a regular file"))
}

/// The refusal a file this command did not generate receives.
fn unmarked_target(t: &Path) -> CliError {
    let shown = t.display();
    CliError::usage(format!(
        "refuses to replace `{shown}`: it was not generated by `clio help --markdown`"
    ))
}

/// The plain-name guard: a page name is one file name inside `--out`, never a path that could
/// leave it. No I/O, so it holds whatever the disk says.
pub(crate) fn page_file_name(file: &str) -> Result<(), CliError> {
    if file.is_empty() || file == "." || file == ".." || file.contains('/') || file.contains('\\') {
        return Err(CliError::usage(format!(
            "page name `{file}` is not a plain file name"
        )));
    }
    Ok(())
}

/// Whether the open `file`'s first line is the generated-by marker.
fn generated(mut file: std::fs::File, target: &Path) -> Result<bool, CliError> {
    use std::io::Read;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| io_error("read", target, &err))?;
    let first_line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    Ok(first_line == GENERATED_MARKER.as_bytes())
}

/// Write `bytes` to the open temp file, apply `mode` in case the mask narrowed it, and sync.
fn write_and_sync(
    temp: &mut std::fs::File,
    bytes: &[u8],
    mode: Mode,
    shown: &Path,
) -> Result<(), CliError> {
    use std::io::Write;

    temp.write_all(bytes)
        .map_err(|err| io_error("write", shown, &err))?;
    fchmod(&*temp, mode).map_err(|err| errno_error("set permissions on", shown, err))?;
    temp.sync_all().map_err(|err| io_error("sync", shown, &err))
}

/// A hidden temp sibling name beside `file`, unique across threads and processes, and bare.
fn temp_name(file: &str) -> OsString {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let (pid, nanos) = (std::process::id(), unix_nanos());
    OsString::from(format!(".{file}.{pid}.{nanos}.{seq}.tmp"))
}

/// The failure a temp create with no free candidate name reports.
fn temp_exhausted(target: &Path) -> CliError {
    let message = format!("failed to create a temp file beside {}", target.display());
    CliError::operational("manual_write", message)
}

/// The failure the armed test seam gives one promote target; always `None` in production.
fn armed_promote_failure(target: &Path) -> Option<CliError> {
    #[cfg(test)]
    {
        let armed = FAILED_PROMOTE_TARGET
            .lock()
            .expect("the promote seam lock is not poisoned");
        if armed.as_deref() == Some(target) {
            let message = format!("the promote step was made to fail for {}", target.display());
            return Some(io_error("promote", target, &std::io::Error::other(message)));
        }
    }
    #[cfg(not(test))]
    let _ = target;
    None
}

/// Move the checked directory to the armed place and put a symlink to the armed decoy directory at
/// its name, once for the `--out` the seam was armed for; a seam armed for another directory stays
/// armed. Always a no-op in production.
fn armed_swap_before_promote(shown: &Path) {
    #[cfg(test)]
    {
        let mut seam = SWAP_BEFORE_PROMOTE
            .lock()
            .expect("the swap seam lock is not poisoned");
        if seam.as_ref().is_some_and(|(armed, _, _)| armed == shown)
            && let Some((_, aside, decoy)) = seam.take()
        {
            let _ = std::fs::rename(shown, aside);
            let _ = std::os::unix::fs::symlink(decoy, shown);
        }
    }
    #[cfg(not(test))]
    let _ = shown;
}

/// Plant a symlink to the armed file outside `--out` at the armed page name, once for the `--out` the
/// seam was armed for; a seam armed for another directory stays armed. Always a no-op in production.
fn armed_plant_after_check(shown: &Path) {
    #[cfg(test)]
    {
        let mut seam = PLANT_AFTER_CHECK
            .lock()
            .expect("the plant seam lock is not poisoned");
        if seam.as_ref().is_some_and(|(armed, _, _)| armed == shown)
            && let Some((_, outside, page)) = seam.take()
        {
            let _ = std::os::unix::fs::symlink(outside, shown.join(page));
        }
    }
    #[cfg(not(test))]
    let _ = shown;
}

pub(crate) fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
}

/// Map an errno from an operation on `path` to the crate error type, as a path failure is.
fn errno_error(action: &str, path: &Path, err: Errno) -> CliError {
    io_error(action, path, &err.into())
}

fn io_error(action: &str, path: &Path, err: &std::io::Error) -> CliError {
    let message = format!("failed to {action} {}: {err}", path.display());
    CliError::operational("manual_write", message)
}

#[cfg(test)]
#[path = "cli_manual_write_dir_tests.rs"]
mod tests;
