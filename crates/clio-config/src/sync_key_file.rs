// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync key file: where it is, how it is
//! created, how it is read back, and the owner-only sidecar that records when it
//! was created.
//!
//! ## Owns
//! - The sync key file path resolution (`CLIO_SYNC_KEY_FILE`, else beside the
//!   encryption key file)
//! - Creating and reading the raw 32-byte sync key file
//! - The owner-only refusal on read, and the `sync-key-created` sidecar
//!
//! ## Does not own
//! - Which store holds the sync key (see `sync_key` and `secret_key_source`)
//! - The OS secret store (see `secret_key_store`)
//! - Generating or fingerprinting key bytes (see `sync_key`)
//!
//! ## Boundary
//!
//! Changes to where the sync key file lives, how it is created, or what the
//! sidecar holds should normally require changes here. Changes to the key bytes
//! or to the store choice should not.
//!
//! Keep this module focused on the sync key file and its sidecar.
//!
//! # Why the sidecar is a second file
//!
//! The key file holds raw bytes on purpose, because the keystore and the file
//! store the same bytes and one length check covers both. A header inside the
//! file would change that shared convention. The creation time is the only value
//! that has no derivation, so it lives beside the file, owner-only like the key
//! itself, and its absence is reported as an unknown creation time rather than
//! mistaken for a lost key. Because the sidecar's name comes from the
//! directory rather than from the key file's name, a new key can inherit the
//! previous key's record, so writing a sidecar replaces whatever was there.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};
use zeroize::Zeroizing;

use crate::secret_key::KEY_LEN;

/// Environment variable naming an explicit sync-key file path.
pub const SYNC_KEY_FILE_ENV: &str = "CLIO_SYNC_KEY_FILE";

/// Sync-key file name used beside the encryption key file.
pub const SYNC_KEY_FILE_NAME: &str = "clio-sync.key";

/// Sidecar file name carrying the creation time of the sync key file.
pub(crate) const CREATED_NAME: &str = "sync-key-created";

/// Resolve the sync-key file path.
///
/// `CLIO_SYNC_KEY_FILE` wins when set. Otherwise the file sits beside the
/// encryption key file — in the same data directory by default, or in the
/// directory an explicit `CLIO_KEY_FILE` names — so one setting moves both keys
/// and the two can never disagree about where the machine keeps key material.
///
/// A blank or whitespace-only value counts as unset, matching every other
/// environment override in this crate.
#[must_use]
pub fn key_file_path<F>(get: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, SYNC_KEY_FILE_ENV) {
        return Some(PathBuf::from(explicit));
    }
    let data_key = crate::secret_key_file::key_file_path(get)?;
    Some(data_key.parent()?.join(SYNC_KEY_FILE_NAME))
}

/// The creation-time sidecar beside the sync-key file.
///
/// Derived from the key file's directory, like the store marker, so the record
/// and the file it describes can never point at two directories.
#[must_use]
pub(crate) fn sidecar_path(key_file: Option<&Path>) -> Option<PathBuf> {
    key_file
        .and_then(Path::parent)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(CREATED_NAME))
}

/// Read and validate the sync key file at `path`.
///
/// The file must be owner-only and hold exactly [`KEY_LEN`] raw bytes. A
/// violation is refused rather than worked around: a key that is short, padded,
/// or readable by another account is not this key, and guessing would lock away
/// everything sealed under the real one.
///
/// # Errors
/// The mode grants group or other access, the file cannot be read, or its length
/// is wrong.
pub(crate) fn read_key(path: &Path) -> Result<Zeroizing<[u8; KEY_LEN]>, AmError> {
    owner_only_refusal(path)?;
    let raw = Zeroizing::new(fs::read(path).map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot read the Clio sync key file {}: {err}. Clio will not generate a \
                 replacement, because a new key cannot open anything sealed under the old one; \
                 restore access to the file and run again",
                path.display()
            ),
        )
    })?);
    if raw.len() != KEY_LEN {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the sync key in {} is {} bytes; expected {KEY_LEN}. Clio will not replace it, \
                 because a new key cannot open anything sealed under the old one; restore the \
                 correct key, or remove the damaged one to start again with everything sealed \
                 under it lost",
                path.display(),
                raw.len()
            ),
        ));
    }
    let mut key = Zeroizing::new([0_u8; KEY_LEN]);
    key.copy_from_slice(&raw);
    Ok(key)
}

/// Create the sync key file with owner-only permissions held from the first byte.
///
/// `create_new` plus the mode in the same `open` call leaves no window in which
/// another user can read the key, and a file that appeared since the existence
/// check is never clobbered.
///
/// # Errors
/// The directory could not be created, or the file could not be opened or written.
pub(crate) fn write_key(path: &Path, key: &[u8; KEY_LEN]) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| io_error(path, "create the directory for", &err))?;
    }
    let mut file = open_new_owner_only(path).map_err(|err| io_error(path, "create", &err))?;
    file.write_all(key)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error(path, "write", &err))
}

/// Remove the sync key file and its sidecar, succeeding when neither exists.
pub(crate) fn remove_key(path: &Path) -> Result<(), AmError> {
    for candidate in [
        path.to_path_buf(),
        sidecar_path(Some(path)).unwrap_or_default(),
    ] {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    format!(
                        "cannot remove the Clio sync key material at {}: {err}",
                        candidate.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Read the creation-time sidecar beside `key_file`.
///
/// An absent sidecar is a missing creation time, never a missing key: the key
/// is untouched and the caller keeps reporting the store, fingerprint, and key
/// count.
///
/// # Errors
/// The sidecar exists but cannot be read.
pub(crate) fn read_created(key_file: Option<&Path>) -> Result<Option<String>, AmError> {
    let Some(path) = sidecar_path(key_file) else {
        return Ok(None);
    };
    match fs::read_to_string(&path) {
        Ok(raw) => {
            let stamp = raw.trim();
            Ok((!stamp.is_empty()).then(|| stamp.to_owned()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot read the Clio sync key creation record {}: {err}",
                path.display()
            ),
        )),
    }
}

/// Write the creation-time sidecar beside `key_file`, owner-only.
///
/// A key file with no directory has nowhere to put the record, so the creation
/// time is simply unavailable there.
///
/// An existing sidecar is removed first, because it describes a key this call is
/// about to replace. The sidecar name is derived from the key file's directory
/// and not from the key file's name, so one that survives a key's removal — a
/// hand `rm`, a failed delete, a restore that carried the key alone, a key file
/// moved inside the same directory — would otherwise be read back as the new
/// key's creation time. A stale creation time is worse than an absent one,
/// because it is what a retention decision would be made on. The record is then
/// created new, so its mode is owner-only from its first byte and whatever
/// occupied the path is never clobbered in place.
///
/// # Errors
/// The filesystem refused the removal or the write.
pub(crate) fn write_created(key_file: Option<&Path>, stamp: &str) -> Result<(), AmError> {
    let Some(path) = sidecar_path(key_file) else {
        return Ok(());
    };
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| io_error(&path, "create the directory for", &err))?;
    }
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot remove the stale Clio sync key creation record {}: {err}",
                    path.display()
                ),
            ));
        }
    }
    let mut file = open_new_owner_only(&path).map_err(|err| io_error(&path, "create", &err))?;
    file.write_all(stamp.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error(&path, "write", &err))
}

/// Refuse a sync key file whose mode grants anything to group or other.
///
/// # Errors
/// [`ErrorCode::Forbidden`] naming the path; never the host's mode value.
#[cfg(unix)]
pub(crate) fn owner_only_refusal(path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|err| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot read the Clio sync key file {}: {err}",
                    path.display()
                ),
            )
        })?
        .permissions()
        .mode();
    if mode.trailing_zeros() >= 6 {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::Forbidden,
        format!(
            "sync key file {}: refused because it is not owner-only; restrict it to the owner \
             (0600) and run again",
            path.display()
        ),
    ))
}

/// Non-Unix platforms have no mode bits to check; the owner-only rule is a
/// no-op there, matching `persist::restrict_dir`.
#[cfg(not(unix))]
pub(crate) fn owner_only_refusal(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

/// A sync-key file that could not be created or written.
fn io_error(path: &Path, action: &str, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot {action} the Clio sync key material {}: {err}. Clio has not stored a new key; \
             fix the path or its permissions and run again",
            path.display()
        ),
    )
}

#[cfg(unix)]
pub(crate) fn open_new_owner_only(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
pub(crate) fn open_new_owner_only(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// A trimmed, non-empty environment value.
fn env_value<F>(get: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "sync_key_file_tests.rs"]
mod tests;
