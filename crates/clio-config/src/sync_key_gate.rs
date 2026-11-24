// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gates key material passes on its way to and
//! from the filesystem: the no-follow openers that produce a descriptor, the
//! judgement of that descriptor, the bytes read through it, and the owner-only
//! rule on the directory that holds the file.
//!
//! ## Owns
//! - [`open_append_no_follow`] and [`open_read_no_follow`]: one open per use,
//!   with `O_NOFOLLOW` and `O_NONBLOCK` on Unix, reporting a link, a refused
//!   descriptor, an absent file, and an open failure as the caller's own message
//! - [`descriptor_refusal`]: the regular-file, owner, and owner-only rule judged
//!   on the descriptor rather than on the path
//! - [`read_bytes`] and [`read_string`]: the contents of a descriptor the gate
//!   already judged
//! - [`ensure_owner_only_dir`]: refusing a linked or non-directory parent before
//!   tightening it to 0700
//!
//! ## Does not own
//! - Where these files live, what they are called, or what they hold (see
//!   `sync_key_file` and `sync_key_rotation`)
//! - The path-based owner-only rule the encryption key read still uses (see
//!   `sync_key_file::owner_only_refusal`)
//!
//! ## Boundary
//!
//! Changes to what makes a file or a directory acceptable to hold key material
//! belong here, once, so the archive, the key file, and the sidecar cannot drift
//! apart. Changes to where they live or what they hold should not.
//!
//! Keep this module focused on the gates themselves.
//!
//! # Why the checks ask the descriptor
//!
//! A check that asks the path can be answered by one file and then used by
//! another: a link, or an entry swapped between the check and the read, gets a
//! verdict for something else. Every caller here opens the path exactly once and
//! judges the descriptor that open returned, so the file the check approved is
//! the file the caller then reads or writes. The callers never re-open by path.
//!
//! # Why the openers neither follow nor block
//!
//! `O_NOFOLLOW` makes the open itself refuse a link at the final name instead of
//! resolving what it points at. `O_NONBLOCK` keeps the open from waiting forever
//! on a FIFO, which is then refused by the file-type check on the descriptor
//! rather than hanging the process before any check can run; `auth_tokens` opens
//! the token store on the same pair of flags and for the same reason.
//!
//! # Why the parent directory is checked too
//!
//! `O_NOFOLLOW` guards the final name only: the directory that name is resolved
//! in can itself be a link, and a `chmod` that followed it would tighten a
//! directory the caller never meant to touch while leaving the real parent loose
//! enough for another account to swap the entry. So the parent is refused when it
//! is a link or not a directory, tightened to 0700 with a failure that is
//! reported rather than swallowed, and read back to prove the mode took.

use std::fs;
use std::path::Path;

use clio_types::{AmError, ErrorCode};

/// Refuse an already-open file that is not a regular owner-only file.
///
/// [`crate::sync_key_file::owner_only_refusal`] asks the path, and a path can
/// lead to a file another account chose. This asks the descriptor, so it judges
/// the file the caller is about to read or write whatever the path pointed at
/// when it was opened. `subject` labels the message, on the same rule as
/// `owner_only_refusal`, so the path in it is what names the file.
///
/// The owner check is explicit — the file's uid against the effective uid of this
/// process — because a successful open does not imply ownership: root opens and
/// appends to any file whatever its mode, and a POSIX ACL can grant this account
/// a 0600 file another account owns. `rustix` is the safe wrapper for that
/// lookup, so this crate's `forbid(unsafe_code)` stays intact.
///
/// # Errors
/// [`ErrorCode::Forbidden`] naming the path when the file is not regular, is
/// owned by another user, or grants group or other access; never the host's uid
/// or mode value.
#[cfg(unix)]
pub(crate) fn descriptor_refusal(
    file: &fs::File,
    path: &Path,
    subject: &str,
) -> Result<(), AmError> {
    descriptor_refusal_for_uid(file, path, subject, effective_uid())
}

/// [`descriptor_refusal`] with the calling user supplied, so the owner rule can
/// be tested without a second account to own the file.
///
/// The production call passes the effective uid of this process and nothing else;
/// splitting it out is what lets the refusal be exercised on one machine by one
/// user, on the same rule `auth_tokens` follows for its token store.
#[cfg(unix)]
fn descriptor_refusal_for_uid(
    file: &fs::File,
    path: &Path,
    subject: &str,
    euid: u32,
) -> Result<(), AmError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = file.metadata().map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!("cannot read the Clio {subject} {}: {err}", path.display()),
        )
    })?;
    let refusal = if !meta.is_file() {
        "it is not a regular file"
    } else if meta.uid() != euid {
        "it is owned by a user other than the one running Clio; change its owner to the one \
         running Clio and run again"
    } else if meta.permissions().mode() & 0o077 != 0 {
        "it is not owner-only; restrict it to the owner (0600) and run again"
    } else {
        return Ok(());
    };
    Err(AmError::new(
        ErrorCode::Forbidden,
        format!("{subject} {}: refused because {refusal}", path.display()),
    ))
}

/// The effective uid of the process, reached through `rustix` so this crate needs
/// no `unsafe` to ask.
#[cfg(unix)]
fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The refusal a link at the final name reports.
///
/// The open refused the link itself, so the file behind it was never opened and
/// nothing was read from or written to it.
#[cfg(unix)]
fn link_refusal(path: &Path, subject: &str) -> AmError {
    AmError::new(
        ErrorCode::Forbidden,
        format!(
            "{subject} {}: refused because it is a symbolic link; replace it with the file \
             itself and run again",
            path.display()
        ),
    )
}

/// Open an existing file for appending without following a link at the path.
///
/// `Ok(None)` means the path holds nothing, which the caller answers by creating
/// the file: the answer comes from the one open and not from a separate existence
/// check that a second lookup could disagree with. The append never creates the
/// file itself, because a creator sets the mode in the same call that creates it
/// (see [`crate::sync_key_file::open_new_owner_only`]).
///
/// `io_error` words the failure for the file the caller names, so the archive
/// says the superseded key was not let go of and the key file says the key has to
/// be restored.
///
/// # Errors
/// [`ErrorCode::Forbidden`] for a link at the final name, for a path holding a
/// shape that cannot hold a key (a FIFO or a directory, both refused before any
/// descriptor exists), or a descriptor [`descriptor_refusal`] refused; the
/// caller's own message for an open that failed for any other reason.
#[cfg(unix)]
pub(crate) fn open_append_no_follow<F>(
    path: &Path,
    subject: &str,
    io_error: F,
) -> Result<Option<fs::File>, AmError>
where
    F: FnOnce(&std::io::Error) -> AmError,
{
    open_no_follow(path, &Access::Append, subject, io_error)
}

/// Open an existing file for reading without following a link at the path.
///
/// `Ok(None)` means the path holds nothing, and the caller answers with an empty
/// archive or a missing-key error rather than with a lookup of its own.
///
/// # Errors
/// The same ways [`open_append_no_follow`] reports.
#[cfg(unix)]
pub(crate) fn open_read_no_follow<F>(
    path: &Path,
    subject: &str,
    io_error: F,
) -> Result<Option<fs::File>, AmError>
where
    F: FnOnce(&std::io::Error) -> AmError,
{
    open_no_follow(path, &Access::Read, subject, io_error)
}

/// The access a no-follow open asks for.
#[cfg(unix)]
enum Access {
    /// Append to an existing file, the way the archive is written.
    Append,
    /// Read an existing file, the way the key file and the archive are read.
    Read,
}

/// One open, no link followed, no FIFO waited on, judged on its descriptor.
#[cfg(unix)]
fn open_no_follow<F>(
    path: &Path,
    access: &Access,
    subject: &str,
    io_error: F,
) -> Result<Option<fs::File>, AmError>
where
    F: FnOnce(&std::io::Error) -> AmError,
{
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    match access {
        Access::Append => options.append(true),
        Access::Read => options.read(true),
    };
    // `O_NOFOLLOW` refuses a link at the final name instead of resolving it;
    // `O_NONBLOCK` keeps the open from waiting forever on a FIFO, which the
    // file-type check on the descriptor then refuses.
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) if err.raw_os_error() == Some(libc::ELOOP) => {
            return Err(link_refusal(path, subject));
        }
        // `O_WRONLY` with `O_NONBLOCK` on a FIFO nobody is reading reports
        // `ENXIO` instead of blocking, so a FIFO planted at the path is refused
        // here rather than waited on. A read open of the same FIFO succeeds and
        // is refused by [`descriptor_refusal`] instead. A directory planted at
        // the path reports `EISDIR` before any descriptor exists to judge, and
        // that is the same refusal: the path holds a shape that cannot hold a key.
        Err(err) if matches!(err.raw_os_error(), Some(libc::ENXIO | libc::EISDIR)) => {
            let shape = if err.raw_os_error() == Some(libc::EISDIR) {
                "a directory is not a file that holds a key"
            } else {
                "a FIFO is a pipe and not a file that holds a key"
            };
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "{subject} {}: refused because it is not a regular file; {shape}, so replace \
                     it with the file itself and run again",
                    path.display()
                ),
            ));
        }
        Err(err) => return Err(io_error(&err)),
    };
    descriptor_refusal(&file, path, subject)?;
    Ok(Some(file))
}

/// Non-Unix platforms have no `O_NOFOLLOW` and no `O_NONBLOCK`, so an append
/// opens the path as it always has; a missing file is still reported as `None`
/// so the caller creates it once.
#[cfg(not(unix))]
pub(crate) fn open_append_no_follow<F>(
    path: &Path,
    _subject: &str,
    io_error: F,
) -> Result<Option<fs::File>, AmError>
where
    F: FnOnce(&std::io::Error) -> AmError,
{
    match fs::OpenOptions::new().append(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error(&err)),
    }
}

/// Non-Unix platforms have no `O_NOFOLLOW` and no `O_NONBLOCK`, so a read opens
/// the path as it always has.
#[cfg(not(unix))]
pub(crate) fn open_read_no_follow<F>(
    path: &Path,
    _subject: &str,
    io_error: F,
) -> Result<Option<fs::File>, AmError>
where
    F: FnOnce(&std::io::Error) -> AmError,
{
    match fs::File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error(&err)),
    }
}

/// Read a descriptor the gate already judged, to its end, as bytes.
///
/// The bytes come from the same open the refusal approved, so no second lookup by
/// path can be answered by a different file.
///
/// # Errors
/// Whatever the read reported.
pub(crate) fn read_bytes(file: &mut fs::File) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    Ok(raw)
}

/// Read a descriptor the gate already judged, to its end, as text.
///
/// The same one-open rule as [`read_bytes`], for the archive's JSON lines. A file
/// that is not valid UTF-8 is an [`std::io::ErrorKind::InvalidData`] error, which
/// is what `fs::read_to_string` reported before this gate existed.
///
/// # Errors
/// Whatever the read reported.
pub(crate) fn read_string(file: &mut fs::File) -> std::io::Result<String> {
    use std::io::Read;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(raw)
}

/// Make the directory holding key material owner-only, or refuse it.
///
/// The caller runs `create_dir_all` first, so a missing parent is created rather
/// than refused. Then, in order: the parent must be a real directory — a link is
/// refused, because `O_NOFOLLOW` on the file's name does not stop the directory
/// that name sits in from being one — it is set to 0700 with a failure that is
/// reported instead of ignored, and it is read back so a mode that did not take is
/// a failure and not an assumption.
///
/// The shared `config::restrict_dir` is deliberately not used here: it ignores
/// the `chmod` error, and a silent failure would leave key material in a
/// directory another account can enter.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when `parent` is a link or not a directory;
/// [`ErrorCode::Internal`] naming the path when the directory cannot be read,
/// cannot be tightened, or still grants group or other access afterwards.
pub(crate) fn ensure_owner_only_dir(parent: &Path) -> Result<(), AmError> {
    let file_type = fs::symlink_metadata(parent)
        .map_err(|err| dir_error(parent, "cannot read the Clio sync key directory", &err))?
        .file_type();
    if file_type.is_symlink() || !file_type.is_dir() {
        let shape = if file_type.is_symlink() {
            "a symbolic link"
        } else {
            "not a directory"
        };
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!(
                "sync key directory {}: refused because it is {shape}; replace it with a real \
                 directory only the owner can enter and run again",
                parent.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|err| {
            dir_error(parent, "cannot restrict the Clio sync key directory", &err)
        })?;
        let mode = fs::symlink_metadata(parent)
            .map_err(|err| dir_error(parent, "cannot read the Clio sync key directory", &err))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "the Clio sync key directory {} still grants {:o} after it was set to 0700, \
                     so Clio will not keep key material in it",
                    parent.display(),
                    mode & 0o777
                ),
            ));
        }
    }
    Ok(())
}

/// A directory that could not be read or tightened.
fn dir_error(path: &Path, action: &str, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{action} {}: {err}", path.display()),
    )
}

#[cfg(test)]
#[path = "sync_key_gate_tests.rs"]
mod tests;
