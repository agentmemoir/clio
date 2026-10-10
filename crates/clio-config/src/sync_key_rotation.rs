// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retention of superseded sync keys: the
//! owner-only archive file they live in, the rotation that fills it, and the
//! newest-first key list a transfer loads.
//!
//! ## Owns
//! - The archive file's name, path, JSON-lines format, and owner-only rule
//! - [`rotate_existing`]: archiving the current key before a new one is
//!   generated, so nothing sealed under it becomes unreadable
//! - [`load_archive_base64`] and [`load_all_base64`], the newest-first lists
//!   the MCP custody read and the sync client feed on
//!
//! ## Does not own
//! - The current key's store, file, marker, sidecar, or fingerprint (see
//!   `sync_key`, `sync_key_file`, and `sync_key_value`)
//! - The OS secret store itself (see `secret_key_store`)
//! - Sealing content with a key (see `clio-sync`'s crypto)
//!
//! ## Boundary
//!
//! Changes to how a superseded key is retained, listed, or ordered should
//! normally require changes here. Changes to how the current key is generated
//! or stored should not.
//!
//! Keep this module focused on superseded-key retention.
//!
//! # Why the archive is a new file beside the key file
//!
//! The current key's store holds exactly one key and is frozen: its file
//! format, its keystore entry, and its marker must not grow a list. A
//! superseded key therefore moves to a file of its own, appended on every
//! rotation, so the store keeps meaning "the current key" and the archive
//! keeps meaning "every key that was ever current here". The file order is
//! the creation order, which makes the list's order stable across runs.
//!
//! # Why rotation archives before it deletes
//!
//! A rotation that deleted first would make a crash between the two steps a
//! data-loss event. The archive entry is written and flushed before the store
//! is asked to let go of the key, so the worst case of an interrupted
//! rotation is a node with only archived keys and no current one, which the
//! operator repairs by running `keygen` again.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::secret_key::KEY_LEN;
use crate::secret_key_source::KeySource;
use crate::secret_key_store::SecretStore;
use crate::sync_key_file as file_store;

/// The owner-only file superseded sync keys are archived into, beside the
/// resolved sync-key file path.
pub(crate) const ARCHIVE_NAME: &str = "clio-sync-keys-archive.json";

/// The store label an archived key reports, distinct from the recorded store
/// the current key names.
pub(crate) const ARCHIVE_STORE_LABEL: &str = "archive file";

/// One superseded sync key as the archive file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchivedSyncKey {
    /// The key as standard base64, the one form the transfer path feeds on.
    pub key_b64: String,
    /// The creation time the store retained for it, when it did.
    pub created_at: Option<String>,
}

/// The archive file beside the resolved sync-key file path.
///
/// Derived from the key file's directory, like the store marker and the
/// creation sidecar, so the record and the keys it describes can never point
/// at two directories.
fn archive_path(key_file: Option<&Path>) -> Option<PathBuf> {
    key_file
        .and_then(Path::parent)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(ARCHIVE_NAME))
}

/// Every archived sync key, oldest first, for a node that has rotated.
///
/// # Errors
/// The archive exists but is not owner-only, cannot be read, or holds a line
/// that is not an archived key. A damaged archive holds key material, so it is
/// refused rather than worked around, on the same rule as the key file.
pub(crate) fn read_entries(key_file: Option<&Path>) -> Result<Vec<ArchivedSyncKey>, AmError> {
    let Some(path) = archive_path(key_file) else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    file_store::owner_only_refusal(&path)?;
    let raw = std::fs::read_to_string(&path).map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot read the Clio sync key archive {}: {err}",
                path.display()
            ),
        )
    })?;
    let mut entries = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let entry: ArchivedSyncKey = serde_json::from_str(line).map_err(|err| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "the Clio sync key archive {} holds a damaged record: {err}. Clio will not \
                     guess at key material; restore the archive from backup",
                    path.display()
                ),
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Append one superseded key to the archive, creating it owner-only when it
/// does not exist.
///
/// The entry is on disk and flushed before this returns, which is what makes
/// archive-before-delete safe.
///
/// # Errors
/// The key file has no directory to hold the archive, or the filesystem
/// refused the write.
pub(crate) fn append_entry(
    key_file: Option<&Path>,
    entry: &ArchivedSyncKey,
) -> Result<(), AmError> {
    let Some(path) = archive_path(key_file) else {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the superseded sync key has nowhere to be archived; set CLIO_SYNC_KEY_FILE to a \
             writable path and run again",
        ));
    };
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot create the directory for the Clio sync key archive {}: {err}",
                    path.display()
                ),
            )
        })?;
    }
    let exists = path.exists();
    let mut file = if exists {
        // The read path refuses an archive that is not owner-only, and the
        // append must refuse it too: the entry is raw key material, so writing
        // it next to whatever could already read the file discloses the key
        // before the next read could refuse.
        file_store::owner_only_refusal(&path)?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|err| archive_io_error(&path, &err))?
    } else {
        file_store::open_new_owner_only(&path).map_err(|err| archive_io_error(&path, &err))?
    };
    let line = serde_json::to_string(entry).expect("serialize archived sync key");
    file.write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|err| archive_io_error(&path, &err))
}

/// An archive write or read that the filesystem refused.
fn archive_io_error(path: &Path, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot write the Clio sync key archive {}: {err}. The superseded key has not been \
             let go of; fix the path or its permissions and run again",
            path.display()
        ),
    )
}

/// Refuse the one custody shape rotation does not take on: a key file
/// shadowing a keystore key.
///
/// A key file beside a keystore key is a second copy of the same secret, so
/// which of them a rotation should archive is ambiguous. This runs before the
/// store choice is confirmed, so a refused run records nothing.
///
/// # Errors
/// [`ErrorCode::Forbidden`]-shaped refusal naming the shadowing file, on the
/// same rule the generation refusal uses.
pub(crate) fn refuse_shadowing_custody(
    source: KeySource,
    file: Option<&Path>,
) -> Result<(), AmError> {
    if source != KeySource::Keystore {
        return Ok(());
    }
    if let Some(path) = file.filter(|path| path.exists()) {
        return Err(crate::sync_key_store::exists_error(&format!(
            "the key file {}",
            path.display()
        )));
    }
    Ok(())
}

/// Archive the current key and let go of it, so a new key can be generated.
///
/// The one definition of the current key follows [`KeySource`]: the key file
/// for the file store, the OS store's entry for the keystore. A key file
/// shadowing a keystore key is a second copy of the same secret and is still
/// refused rather than rotated, on the same rule the generation refusal uses.
///
/// # Errors
/// The current key cannot be read, the archive refused the entry, or the store
/// refused the removal. Nothing is deleted unless the archive write succeeded.
pub(crate) fn rotate_existing(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<&Path>,
) -> Result<(), AmError> {
    match source {
        KeySource::File => {
            let Some(path) = file else {
                return Ok(());
            };
            if !path.exists() {
                return Ok(());
            }
            let key = file_store::read_key(path)?;
            let created_at = file_store::read_created(Some(path))?;
            append_entry(
                file,
                &ArchivedSyncKey {
                    key_b64: base64::engine::general_purpose::STANDARD.encode(key.as_slice()),
                    created_at,
                },
            )?;
            file_store::remove_key(path)
        }
        KeySource::Keystore => {
            if let Some(path) = file.filter(|path| path.exists()) {
                return Err(crate::sync_key_store::exists_error(&format!(
                    "the key file {}",
                    path.display()
                )));
            }
            let Some(raw) = store.get()? else {
                return Ok(());
            };
            if raw.len() != KEY_LEN {
                return Err(AmError::new(
                    ErrorCode::ConfigCorrupt,
                    format!(
                        "the sync key in the OS secret store is {} bytes; expected {KEY_LEN}. \
                         Clio will not rotate it, because a replacement cannot open anything \
                         sealed under the real one; restore the correct key",
                        raw.len()
                    ),
                ));
            }
            let created_at = store
                .attributes()?
                .get(crate::sync_key_value::CREATED_ATTRIBUTE)
                .cloned();
            append_entry(
                file,
                &ArchivedSyncKey {
                    key_b64: base64::engine::general_purpose::STANDARD.encode(raw.as_slice()),
                    created_at,
                },
            )?;
            store.delete()
        }
    }
}

/// The archived keys, newest first, as the transfer path feeds them.
///
/// # Errors
/// The archive exists but cannot be read, on the same rule [`read_entries`]
/// reports.
pub fn load_archive_base64<F>(get: &F) -> Result<Vec<String>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    let mut out = Vec::new();
    for entry in read_entries(file.as_deref())?.into_iter().rev() {
        out.push(entry.key_b64);
    }
    Ok(out)
}

/// Every key this node holds, newest first: the store-backed current key,
/// then the archived keys. The list feeds the sync client, which seals with
/// the first entry and opens with any of them.
///
/// The explicit file named by `CLIO_SYNC_KEY_FILE` is not handled here; the
/// MCP custody helper reads that file itself and prepends it to
/// [`load_archive_base64`].
///
/// # Errors
/// The recorded store is unreachable, an existing key file is damaged, or the
/// archive cannot be read.
pub fn load_all_base64<F>(get: &F) -> Result<Vec<String>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = Vec::new();
    if let Some(key) = crate::sync_key::load_base64(get)? {
        out.push(key);
    }
    out.extend(load_archive_base64(get)?);
    Ok(out)
}

#[cfg(test)]
#[path = "sync_key_rotation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_key_rotation_gen_tests.rs"]
mod gen_tests;
