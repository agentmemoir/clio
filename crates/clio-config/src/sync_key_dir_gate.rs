// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one rule the directory holding key
//! material has to pass: it is refused unless it is a real directory, and it is
//! tightened to `0700` through the same opened descriptor that was judged rather
//! than through its path.
//!
//! ## Owns
//! - [`ensure_owner_only_dir`]: the early return for a path that names no
//!   directory of ours, the refusal of a link or a non-directory parent, the
//!   `0700` that is set and read back, and the refusal of a directory that was
//!   swapped for another one after it was opened
//! - [`open_key_dir`] and [`tighten_dir`]: the open and the mode change, kept
//!   apart so a test can swap the entry between them
//! - [`shape_refusal`], [`replaced_refusal`], and [`dir_error`]: the three
//!   outcomes, worded once
//!
//! ## Does not own
//! - The file gates: the no-follow openers, the descriptor refusal, and the
//!   reads through a judged descriptor (see `sync_key_gate`)
//! - Where the key file lives or what it holds (see `sync_key_file` and
//!   `sync_key_rotation`)
//!
//! ## Boundary
//!
//! Changes to what makes a directory acceptable to hold key material belong
//! here, once. Changes to where the directory is, or to what is kept in it,
//! should not.
//!
//! Keep this module focused on the directory gate.
//!
//! # Why one descriptor for the whole decision
//!
//! A path lookup can be answered by one object and then used by another. Three
//! lookups of the same path — one to judge the type, one to set the mode, one to
//! read the mode back — let a user who can write the parent of the key directory
//! replace the entry between any two of them, so `0700` could land on a
//! directory Clio never judged and the read-back could approve a third object.
//!
//! So the directory is opened once with `O_NOFOLLOW` and `O_DIRECTORY`, and that
//! descriptor is held for the whole decision: the type is the kernel's answer at
//! the open, the mode is set on the descriptor, the mode is read back from the
//! descriptor, and the path is compared against the descriptor's own `st_dev`
//! and `st_ino` at the end so a directory swapped underneath is refused rather
//! than trusted. `rustix` is the safe wrapper for all of it, so this crate's
//! `forbid(unsafe_code)` stays intact.

use std::fs;
use std::path::Path;

use clio_types::{AmError, ErrorCode};

use crate::sync_key_gate::report_refusal;

/// The failure an unreadable directory reports.
const READ_ACTION: &str = "cannot read the Clio sync key directory";

/// The failure an untightenable directory reports.
const RESTRICT_ACTION: &str = "cannot restrict the Clio sync key directory";

/// What a refusal says about a link at the directory's final name.
const LINK_SHAPE: &str = "a symbolic link";

/// What a refusal says about a parent that is not a directory at all.
const NOT_DIR_SHAPE: &str = "not a directory";

/// Make the directory holding key material owner-only, or refuse it.
///
/// Clio tightens the directory it is given only when that directory is a real,
/// named directory, and never the process working directory or a parent of it: a
/// path that is `.`, is empty, or is made only of `..` names a directory no key
/// setting chose, so it is left exactly as it is.
///
/// The caller runs `create_dir_all` first, so a missing parent is created rather
/// than refused. Then, in order: the parent is opened once without following a
/// link and without accepting anything but a directory, so a link is refused by
/// the kernel rather than by a second lookup; `0700` is set on that descriptor
/// with a failure that is reported instead of ignored; the mode is read back from
/// the same descriptor so a mode that did not take is a failure and not an
/// assumption; and the path is compared against the descriptor's `st_dev` and
/// `st_ino`, so a directory replaced after the open is refused instead of
/// tightened.
///
/// The shared `config::restrict_dir` is deliberately not used here: it ignores
/// the `chmod` error, and a silent failure would leave key material in a
/// directory another account can enter.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when `parent` is a link, is not a directory, or was
/// replaced while it was being checked; [`ErrorCode::Internal`] naming the path
/// when the directory cannot be opened, cannot be tightened, or still grants
/// group or other access afterwards.
pub(crate) fn ensure_owner_only_dir(parent: &Path) -> Result<(), AmError> {
    if !parent
        .components()
        .any(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Ok(());
    }
    // Unix holds one descriptor for the whole decision. A platform without
    // `O_NOFOLLOW` and `O_DIRECTORY` has no descriptor to hold, so it judges the
    // path once, as it always has.
    #[cfg(unix)]
    {
        let dir = open_key_dir(parent)?;
        tighten_dir(&dir, parent)
    }
    #[cfg(not(unix))]
    {
        judge_dir(parent)
    }
}

/// Open the directory that holds key material, once, without following a link.
///
/// `DIRECTORY` makes the open refuse a parent that is not a directory (`ENOTDIR`)
/// and `NOFOLLOW` makes it refuse one that is a link, both before any mode is
/// touched, so the type of what Clio is about to tighten is the kernel's answer at
/// this open and no second lookup is asked. `CLOEXEC` keeps the descriptor out of
/// a child process.
///
/// Linux reports `ENOTDIR` for a link here, because `O_DIRECTORY` is checked
/// before `O_NOFOLLOW` is honoured, and other hosts report `ELOOP`. Both are
/// refused with no descriptor open and nothing tightened, so the shape is read
/// once more only to word the refusal: no verdict depends on that lookup, and it
/// runs only on the path that was already refused.
///
/// # Errors
/// [`ErrorCode::Forbidden`] for a link or a parent that is not a directory;
/// [`ErrorCode::Internal`] naming the path when the open failed for any other
/// reason, which includes a directory that is not there.
#[cfg(unix)]
fn open_key_dir(parent: &Path) -> Result<std::os::fd::OwnedFd, AmError> {
    use rustix::fs::{Mode, OFlags, open};
    match open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(dir) => Ok(dir),
        Err(err) if err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR => {
            Err(shape_refusal(parent, refused_shape(parent)))
        }
        Err(err) => Err(dir_error(parent, READ_ACTION, &err.into())),
    }
}

/// The shape a refused path has, for the wording of the refusal only.
#[cfg(unix)]
fn refused_shape(parent: &Path) -> &'static str {
    let is_link = fs::symlink_metadata(parent).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link { LINK_SHAPE } else { NOT_DIR_SHAPE }
}

/// Set `0700` through the held descriptor, read it back, and prove it is still
/// the directory the path names.
///
/// Every step asks the descriptor, so the directory whose mode changes is the
/// directory that was opened whatever the path has held since. The last step is
/// the one that cannot be answered from the descriptor: the path is looked up
/// once more and its `st_dev` and `st_ino` are compared with the held
/// descriptor's, so a directory swapped in after the open is a refusal and not a
/// directory Clio reports as tightened.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the path now names a different entry;
/// [`ErrorCode::Internal`] naming the path when the descriptor cannot be read,
/// cannot be tightened, or still grants group or other access afterwards.
#[cfg(unix)]
fn tighten_dir(dir: &std::os::fd::OwnedFd, parent: &Path) -> Result<(), AmError> {
    use rustix::fs::{Mode, fchmod, fstat};
    let held = fstat(dir).map_err(|err| dir_error(parent, READ_ACTION, &err.into()))?;
    fchmod(dir, Mode::from_bits_truncate(0o700))
        .map_err(|err| dir_error(parent, RESTRICT_ACTION, &err.into()))?;
    let mode = Mode::from_raw_mode(
        fstat(dir)
            .map_err(|err| dir_error(parent, READ_ACTION, &err.into()))?
            .st_mode,
    );
    if mode.bits() & 0o077 != 0 {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "the Clio sync key directory {} still grants {:o} after it was set to 0700, \
                 so Clio will not keep key material in it",
                parent.display(),
                mode.bits() & 0o777
            ),
        ));
    }
    if still_the_held_entry(parent, &held)? {
        return Ok(());
    }
    Err(replaced_refusal(parent))
}

/// Whether the path still names the entry the held descriptor names.
///
/// One lookup and one comparison of `st_dev` and `st_ino`: a second directory at
/// the same name is a different inode, and a link planted there is refused by the
/// `SYMLINK_NOFOLLOW` lookup reporting the link's own inode.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the path when it cannot be read at all.
#[cfg(unix)]
fn still_the_held_entry(parent: &Path, held: &rustix::fs::Stat) -> Result<bool, AmError> {
    use std::os::unix::fs::MetadataExt;
    let now = fs::symlink_metadata(parent).map_err(|err| dir_error(parent, READ_ACTION, &err))?;
    Ok(now.dev() == held.st_dev && now.ino() == held.st_ino)
}

/// Judge the parent by its one path lookup, for a platform with no descriptor to
/// hold.
///
/// This is the rule Unix had before the descriptor: a link is refused, and so is
/// anything that is not a directory. There is no `O_NOFOLLOW` here to refuse the
/// link at the open, and no `fchmod` to set the mode through a descriptor, so the
/// mode is left alone rather than set through a path that a second lookup would
/// have to trust again.
///
/// # Errors
/// [`ErrorCode::Forbidden`] for a link or a parent that is not a directory;
/// [`ErrorCode::Internal`] naming the path when it cannot be read.
#[cfg(not(unix))]
fn judge_dir(parent: &Path) -> Result<(), AmError> {
    let file_type = fs::symlink_metadata(parent)
        .map_err(|err| dir_error(parent, READ_ACTION, &err))?
        .file_type();
    if file_type.is_symlink() {
        return Err(shape_refusal(parent, LINK_SHAPE));
    }
    if !file_type.is_dir() {
        return Err(shape_refusal(parent, NOT_DIR_SHAPE));
    }
    Ok(())
}

/// The refusal a link or a non-directory parent reports.
fn shape_refusal(parent: &Path, shape: &str) -> AmError {
    report_refusal("sync key directory", parent, &format!("it is {shape}"));
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "sync key directory {}: refused because it is {shape}; replace it with a real \
             directory only the owner can enter and run again",
            parent.display()
        ),
    )
}

/// The refusal a directory swapped after its open reports.
///
/// The mode was set on the descriptor, so the directory Clio opened is the one it
/// tightened; the name, meanwhile, holds something Clio never judged, so the
/// directory the caller will write into is not the directory that was approved.
fn replaced_refusal(parent: &Path) -> AmError {
    report_refusal(
        "sync key directory",
        parent,
        "it was replaced while Clio was checking it",
    );
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "sync key directory {}: refused because it was replaced while Clio was checking \
             it; replace it with a real directory only the owner can enter and run again",
            parent.display()
        ),
    )
}

/// A directory that could not be read or tightened.
fn dir_error(path: &Path, action: &str, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{action} {}: {err}", path.display()),
    )
}

#[cfg(test)]
#[path = "sync_key_dir_gate_tests.rs"]
mod tests;
