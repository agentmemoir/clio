// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one rule the directory holding key
//! material has to pass: it is refused unless it is a real directory owned by the
//! user running Clio, and it is tightened to `0700` through the same opened
//! descriptor that was judged rather than through its path.
//!
//! ## Owns
//! - [`ensure_owner_only_dir`]: the early return for a path that names no
//!   directory of ours, the refusal of a link or a non-directory parent, the
//!   owner refusal, the `0700` that is set and read back, and the refusal of a
//!   directory that was swapped for another one after it was opened
//! - [`open_key_dir`] and [`tighten_dir`]: the open and the mode change, kept
//!   apart so a test can swap the entry between them
//! - [`shape_refusal`], [`replaced_refusal`], [`owner_refusal`], and [`dir_error`]:
//!   the four outcomes, worded once
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
//! descriptor is held for the whole decision: the type, the owner, and the mode
//! are the held object's own, the mode is set and read back through the
//! descriptor, and the path is compared against the descriptor's `st_dev` and
//! `st_ino` at the end so a directory swapped underneath is refused rather than
//! trusted. `rustix` is the safe wrapper for all of it, so this crate's
//! `forbid(unsafe_code)` stays intact.
//!
//! # Why the open asks for no permission at all
//!
//! The descriptor is opened with `O_PATH`, which the kernel grants on a
//! directory without consulting its mode, so a key directory at `0300` or `0100`
//! — owner search without owner read — is opened and repaired. A read open would
//! answer `EACCES` for exactly the modes the path-based `chmod` this gate
//! replaced used to fix, which would turn a state Clio repairs into a refusal.
//!
//! `O_PATH` also has no `fchmod`: the mode is set with `chmodat` relative to the
//! held descriptor, and, for a directory that grants the owner no search to
//! resolve `"."` through, through the descriptor's own `/proc/self/fd` entry,
//! which names the held inode and not the path Clio was given.
//!
//! # What is still reached by path
//!
//! The ancestors above the key directory are resolved by the kernel on the open
//! and are not held, and the caller creates the key file by path after this gate
//! returns. Both windows are older than this module and are stated as limits
//! rather than closed here.

#[cfg(unix)]
use std::os::fd::OwnedFd;
use std::path::Path;

use clio_types::{AmError, ErrorCode};

#[cfg(unix)]
use crate::sync_key_gate::FOREIGN_OWNER;
use crate::sync_key_gate::report_refusal;

/// The failure an unreadable directory reports.
const READ_ACTION: &str = "cannot read the Clio sync key directory";

/// The failure an untightenable directory reports.
#[cfg(unix)]
const RESTRICT_ACTION: &str = "cannot restrict the Clio sync key directory";

/// What a refusal says about a link at the directory's final name.
const LINK_SHAPE: &str = "a symbolic link";

/// What a refusal says about a parent that is not a directory at all.
const NOT_DIR_SHAPE: &str = "not a directory";

/// Make the directory holding key material owner-only, or refuse it.
///
/// Clio tightens the directory it is given only when that directory is a real,
/// named directory owned by the user running Clio, and never the process working
/// directory or a parent of it: a path that is `.`, is empty, or is made only of
/// `..` names a directory no key setting chose, so it is left exactly as it is.
///
/// The caller runs `create_dir_all` first, so a missing parent is created rather
/// than refused. Then, in order: the parent is opened once without following a
/// link and without accepting anything but a directory, so a link is refused by
/// the kernel rather than by a second lookup; the held object is refused if it is
/// owned by another user; `0700` is set through that descriptor, where a failure
/// is reported instead of ignored; the mode is read back from the same descriptor
/// so a mode that did not take is a failure and not an assumption; and the path
/// is compared against the descriptor's `st_dev` and `st_ino`, so a directory
/// replaced after the open is refused instead of tightened.
///
/// A path-based mode change whose error is ignored is deliberately not used
/// here: a silent failure would leave key material in a directory another
/// account can enter, so the mode is set through the held descriptor and read
/// back.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when `parent` is a link, is not a directory, is owned
/// by another user, or was replaced while it was being checked;
/// [`ErrorCode::Internal`] naming the path when the directory cannot be opened,
/// cannot be tightened, or still grants group or other access afterwards.
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

/// The flags the key directory is opened with.
///
/// `DIRECTORY` makes the open refuse a parent that is not a directory
/// (`ENOTDIR`) and `NOFOLLOW` makes it refuse one that is a link, both before any
/// mode is touched, so the type of what Clio is about to tighten is the kernel's
/// answer at this open and no second lookup is asked. `CLOEXEC` keeps the
/// descriptor out of a child process.
///
/// `PATH` asks for no access at all, which is what lets a directory at `0300` or
/// `0100` be opened and repaired: those are modes the path-based `chmod` this
/// gate replaced used to fix, and a read open reports `EACCES` for them. A
/// platform with no `O_PATH` opens read-only, as the gate always has.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn held_flags() -> rustix::fs::OFlags {
    use rustix::fs::OFlags;
    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn held_flags() -> rustix::fs::OFlags {
    use rustix::fs::OFlags;
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Open the directory that holds key material, once, without following a link.
///
/// One open, and the descriptor it returns is the object every later step asks:
/// nothing here looks at the path again except to word a refusal and, at the very
/// end, to prove the name still holds what was opened.
///
/// Linux reports `ENOTDIR` for a link here, because `O_DIRECTORY` is checked
/// before `O_NOFOLLOW` is honoured, and other hosts report `ELOOP`. Both are
/// refused with no descriptor open and nothing tightened, so the shape is read
/// once more only to word the refusal: no verdict depends on that lookup, and it
/// runs only on the path that was already refused. That lookup can only speak for
/// this path when the open failed on its final component; when an ancestor is the
/// component that is not a directory, the lookup cannot answer at all and the
/// failure is reported as the unreadable path it is rather than as a claim about
/// the key directory's own shape.
///
/// # Errors
/// [`ErrorCode::Forbidden`] for a link or a parent that is not a directory;
/// [`ErrorCode::Internal`] naming the path when the open failed for any other
/// reason, which includes a directory that is not there and a path that cannot be
/// looked up at all.
#[cfg(unix)]
fn open_key_dir(parent: &Path) -> Result<OwnedFd, AmError> {
    use rustix::fs::{Mode, open};
    match open(parent, held_flags(), Mode::empty()) {
        Ok(dir) => Ok(dir),
        Err(err) if err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR => {
            match refused_shape(parent) {
                Some(shape) => Err(shape_refusal(parent, shape)),
                None => Err(dir_error(parent, READ_ACTION, &err.into())),
            }
        }
        Err(err) => Err(dir_error(parent, READ_ACTION, &err.into())),
    }
}

/// The shape a refused path has, for the wording of the refusal only.
///
/// `None` means the lookup could not answer for this path: an ancestor that is not
/// a directory refuses the open and this lookup alike, so the final component was
/// never seen and the refusal must not claim anything about it.
#[cfg(unix)]
fn refused_shape(parent: &Path) -> Option<&'static str> {
    let meta = std::fs::symlink_metadata(parent).ok()?;
    if meta.file_type().is_symlink() {
        Some(LINK_SHAPE)
    } else {
        Some(NOT_DIR_SHAPE)
    }
}

/// Set `0700` through the held descriptor, read it back, and prove it is still
/// the directory the path names.
#[cfg(unix)]
fn tighten_dir(dir: &OwnedFd, parent: &Path) -> Result<(), AmError> {
    tighten_dir_for_uid(dir, parent, crate::sync_key_gate::effective_uid())
}

/// [`tighten_dir`] with the calling user supplied, so the owner rule can be
/// exercised by one account on one machine, the way the file gate's owner rule is.
///
/// Every step asks the descriptor, so the directory whose mode changes is the
/// directory that was opened whatever the path has held since. The owner is
/// judged before the mode is touched, so a directory belonging to another user is
/// refused with its mode left alone instead of being reported as one Clio could
/// not restrict. The last step is the one that cannot be answered from the
/// descriptor: the path is looked up once more and its `st_dev` and `st_ino` are
/// compared with the held descriptor's, so a directory swapped in after the open
/// is a refusal and not a directory Clio reports as tightened.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the held directory is owned by another user or
/// the path now names a different entry; [`ErrorCode::Internal`] naming the path
/// when the descriptor cannot be read, cannot be tightened, or still grants group
/// or other access afterwards.
#[cfg(unix)]
fn tighten_dir_for_uid(dir: &OwnedFd, parent: &Path, euid: u32) -> Result<(), AmError> {
    use rustix::fs::{Mode, fstat};
    let held = fstat(dir).map_err(|err| dir_error(parent, READ_ACTION, &err.into()))?;
    if held.st_uid != euid {
        return Err(owner_refusal(parent));
    }
    restrict_held(dir, parent)?;
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

/// Set `0700` on the directory the descriptor names.
///
/// `chmodat` with `"."` relative to the descriptor is the mode change, and it
/// asks the kernel for no second lookup of the path Clio was given. A directory
/// the owner cannot search — `0000`, `0600`, and every other mode without the
/// owner's execute bit — refuses `"."` with `EACCES`, which is a mode the
/// path-based `chmod` this gate replaced used to repair, so that refusal falls
/// back to the descriptor's own `/proc/self/fd` entry.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the path when the mode could not be set.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn restrict_held(dir: &OwnedFd, parent: &Path) -> Result<(), AmError> {
    use rustix::fs::{AtFlags, Mode, chmodat};
    match chmodat(dir, ".", Mode::from_bits_truncate(0o700), AtFlags::empty()) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::ACCESS) => restrict_through_held_handle(dir, parent),
        Err(err) => Err(dir_error(parent, RESTRICT_ACTION, &err.into())),
    }
}

/// Set `0700` on the directory a descriptor that cannot be `fchmod`'d names.
///
/// `/proc/self/fd/<n>` is a magic link to the inode the descriptor holds, so the
/// mode still lands on the object the open judged: resolving it does not look up
/// the path Clio was given, and nothing standing at that path can receive the
/// change.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the path when the mode could not be set, which
/// includes a host with no `procfs` mounted.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn restrict_through_held_handle(dir: &OwnedFd, parent: &Path) -> Result<(), AmError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    let held = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o700))
        .map_err(|err| dir_error(parent, RESTRICT_ACTION, &err))
}

/// Set `0700` on the directory the descriptor names, on a Unix with no `O_PATH`.
///
/// The descriptor here is a read descriptor, which `fchmod` accepts, so the mode
/// lands on the object the open judged and the path is not looked up again. A
/// directory the owner cannot read is refused by the open itself on such a host.
///
/// # Errors
/// [`ErrorCode::Internal`] naming the path when the mode could not be set.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn restrict_held(dir: &OwnedFd, parent: &Path) -> Result<(), AmError> {
    use rustix::fs::{Mode, fchmod};
    fchmod(dir, Mode::from_bits_truncate(0o700))
        .map_err(|err| dir_error(parent, RESTRICT_ACTION, &err.into()))
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
    let now =
        std::fs::symlink_metadata(parent).map_err(|err| dir_error(parent, READ_ACTION, &err))?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let same_dev = now.dev() == held.st_dev;
    // macOS reports st_dev as i32, but a device number is never negative.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    #[allow(clippy::cast_sign_loss)]
    let same_dev = now.dev() == held.st_dev as u64;
    Ok(same_dev && now.ino() == held.st_ino)
}

/// Judge the parent by its one path lookup, for a platform with no descriptor to
/// hold.
///
/// This is the rule Unix had before the descriptor: a link is refused, and so is
/// anything that is not a directory. There is no `O_NOFOLLOW` here to refuse the
/// link at the open, and no descriptor to set the mode through, so the mode is
/// left alone rather than set through a path that a second lookup would have to
/// trust again.
///
/// # Errors
/// [`ErrorCode::Forbidden`] for a link or a parent that is not a directory;
/// [`ErrorCode::Internal`] naming the path when it cannot be read.
#[cfg(not(unix))]
fn judge_dir(parent: &Path) -> Result<(), AmError> {
    let file_type = std::fs::symlink_metadata(parent)
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

/// The refusal a directory owned by another user reports.
///
/// The wording is the file gate's own, so the owner rule reads the same whether it
/// is a file or the directory holding key material, and the mode is left exactly
/// as it was: nothing was set on a directory this account does not own.
///
/// Unix only, next to its one caller: a platform with no descriptor to hold has no
/// owner to compare on it.
#[cfg(unix)]
fn owner_refusal(parent: &Path) -> AmError {
    report_refusal("sync key directory", parent, FOREIGN_OWNER);
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "sync key directory {}: refused because {FOREIGN_OWNER}",
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
