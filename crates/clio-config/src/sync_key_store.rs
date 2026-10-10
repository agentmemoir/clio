// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync key lifecycle against a store the
//! caller supplies: loading, generating, deleting, and reporting from either
//! the file store or the OS secret store, without resolving any path or
//! reading any environment.
//!
//! ## Owns
//! - [`load_with`], [`generate_with`], [`delete_with`], and [`report_with`],
//!   the store-injected variants the env-driven lifecycle and the tests share
//! - The refusal for a store that already holds a key
//! - Extending a report with the archived keys
//!
//! ## Does not own
//! - Environment and precedence resolution; that chain was removed, and the
//!   MCP custody read lives in `clio-mcp`'s `sync_tools`
//! - The archive file and rotation (see `sync_key_rotation`)
//! - The key file, its path, or its sidecar (see `sync_key_file`)
//! - The OS secret store itself (see `secret_key_store`)
//!
//! ## Boundary
//!
//! Changes to how a key moves in or out of a store the caller named should
//! normally require changes here. Changes to which store is named should not.
//!
//! Keep this module focused on store-injected lifecycle variants.

use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};
use zeroize::Zeroizing;

use crate::auth_tokens::now_utc;
use crate::secret_key_source::KeySource;
use crate::secret_key_store::SecretStore;
use crate::sync_key_file as file_store;
use crate::sync_key_file::SYNC_KEY_FILE_ENV;
use crate::sync_key_rotation as rotation;
use crate::sync_key_value::{
    CREATED_ATTRIBUTE, LoadedSyncKey, SyncKeyLocation, SyncKeyReport, empty_report, generate_bytes,
    key_from_bytes, report_of,
};

/// Load from a store the caller supplies, for tests and for one-shot callers.
///
/// # Errors
/// The key exists but cannot be used.
pub fn load_with(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<PathBuf>,
) -> Result<Option<LoadedSyncKey>, AmError> {
    match source {
        KeySource::File => {
            let Some(path) = file else {
                return Ok(None);
            };
            if !path.exists() {
                return Ok(None);
            }
            let key = file_store::read_key(&path)?;
            Ok(Some(LoadedSyncKey::new(key, SyncKeyLocation::File(path))))
        }
        KeySource::Keystore => match store.get()? {
            None => Ok(None),
            Some(raw) => {
                let location = SyncKeyLocation::Keystore;
                let key = key_from_bytes(&Zeroizing::new(raw), &location)?;
                Ok(Some(LoadedSyncKey::new(key, location)))
            }
        },
    }
}

/// Refuse when the store, or a key file shadowing it, already holds a key.
///
/// The one definition of "already exists", run both before the store choice is
/// recorded and inside the write, because a record written on the way to a
/// refusal names a store no key ever reached. A shadowing file counts for both
/// stores: a key file beside a keystore key is a second copy of the same secret.
fn refuse_if_present(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<&Path>,
) -> Result<(), AmError> {
    if let Some(path) = file.filter(|path| path.exists()) {
        return Err(exists_error(&format!("the key file {}", path.display())));
    }
    if source == KeySource::Keystore && store.get()?.is_some() {
        return Err(exists_error("the OS secret store"));
    }
    Ok(())
}

/// Generate into a store the caller supplies.
///
/// # Errors
/// The same failures [`generate`] reports.
pub fn generate_with(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<PathBuf>,
) -> Result<LoadedSyncKey, AmError> {
    refuse_if_present(store, source, file.as_deref())?;
    match source {
        KeySource::File => {
            let path = file.ok_or_else(|| {
                AmError::new(
                    ErrorCode::ConfigCorrupt,
                    format!(
                        "the sync key is kept in a file, but no key-file path could be resolved; \
                         set {SYNC_KEY_FILE_ENV} to a writable path and run again"
                    ),
                )
            })?;
            let key = generate_bytes();
            file_store::write_key(&path, &key)?;
            // Best effort, for the reason the keystore's attribute write gives:
            // the key is already on disk, so failing here would report a
            // generation that happened and withhold the key from the one command
            // that can still show it. A sidecar that could not be written is
            // reported as an unknown creation time, which is the honest answer
            // and the one the report already knows how to give.
            let _ = file_store::write_created(Some(&path), &now_utc());
            Ok(LoadedSyncKey::new(key, SyncKeyLocation::File(path)))
        }
        KeySource::Keystore => {
            let key = generate_bytes();
            store.set(key.as_slice())?;
            // The platform may ignore the attribute without error (macOS and
            // Windows), so the value is written and then read back; an absent
            // attribute is an unavailable creation time, never an error. A store
            // that *refuses* the write is left alone for the same reason the key
            // itself is: failing here would report a generation that succeeded
            // and withhold the key, which is how a key becomes unrecoverable.
            // The refusal is not lost, because `report_with` reads the same
            // attribute back and propagates the store's error, so the operator
            // sees "the store refused" rather than "this platform keeps none".
            let _ = store.set_attribute(CREATED_ATTRIBUTE, &now_utc());
            Ok(LoadedSyncKey::new(key, SyncKeyLocation::Keystore))
        }
    }
}

/// Delete from a store the caller supplies.
///
/// # Errors
/// The store or the filesystem refused the removal.
pub fn delete_with(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<PathBuf>,
) -> Result<(), AmError> {
    match source {
        KeySource::File => match file {
            Some(path) => file_store::remove_key(&path),
            None => Ok(()),
        },
        KeySource::Keystore => store.delete(),
    }
}

/// Report from a store the caller supplies.
///
/// # Errors
/// The recorded store is unreachable, an existing key file or archive is
/// damaged, or the keystore refused to hand over its attributes. That last one
/// is a refusal rather than a missing creation time: an empty attribute map
/// means the platform keeps none, and a store that cannot answer is a different
/// fact.
pub fn report_with(
    store: &dyn SecretStore,
    source: KeySource,
    file: Option<PathBuf>,
) -> Result<SyncKeyReport, AmError> {
    match source {
        KeySource::File => {
            let Some(path) = file else {
                // No resolved path means no archive either: the archive lives
                // beside the resolved key-file path, so there is nowhere to
                // read archived keys from and the report is truly empty.
                return Ok(empty_report(source));
            };
            if !path.exists() {
                // The current key file is gone, but its directory can still
                // hold the archive an interrupted rotation left behind, so the
                // report still shows those keys.
                return with_archived(empty_report(source), Some(&path));
            }
            let key = file_store::read_key(&path)?;
            let created_at = file_store::read_created(Some(&path))?;
            Ok(with_archived(
                report_of(&key, source, created_at),
                Some(&path),
            )?)
        }
        KeySource::Keystore => match store.get()? {
            // The store holds no current key, but the archive beside the
            // resolved key-file path still holds every superseded one: the
            // state an interrupted rotation leaves behind.
            None => with_archived(empty_report(source), file.as_deref()),
            Some(raw) => {
                let location = SyncKeyLocation::Keystore;
                let key = key_from_bytes(&Zeroizing::new(raw), &location)?;
                // Read back rather than trust the write: a store that ignores
                // the attribute returns success and stores nothing, and that is
                // an unavailable creation time. A store that cannot answer at
                // all is an error, so the two never look the same.
                let attributes = store.attributes()?;
                let created_at = attributes.get(CREATED_ATTRIBUTE).cloned();
                Ok(with_archived(
                    report_of(&key, source, created_at),
                    file.as_deref(),
                )?)
            }
        },
    }
}

/// Extend a one-key report with the archived keys, oldest last, and set the
/// count to every key held.
///
/// # Errors
/// The archive exists but cannot be read, on the same rule the rotation
/// reports; an archived key that is damaged is refused rather than guessed at.
fn with_archived(
    mut report: crate::sync_key_value::SyncKeyReport,
    file: Option<&std::path::Path>,
) -> Result<crate::sync_key_value::SyncKeyReport, AmError> {
    // The archive file is oldest first; the report is newest first, like the
    // key list a transfer loads.
    for entry in rotation::read_entries(file)?.into_iter().rev() {
        report.keys.push(crate::sync_key_value::SyncKeyEntry {
            fingerprint: crate::sync_key_value::fingerprint_of_base64(&entry.key_b64)?,
            created_at: entry.created_at,
            current: false,
            store: rotation::ARCHIVE_STORE_LABEL.to_owned(),
        });
    }
    report.count = report.keys.len();
    Ok(report)
}

/// The refusal for a store that already holds a key.
pub(crate) fn exists_error(where_: &str) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "a sync key already exists in {where_}; Clio will not replace it, because a new key \
             cannot open anything sealed under the old one. Run `clio sync key export` to read \
             the key"
        ),
    )
}

#[cfg(test)]
#[path = "sync_key_store_tests.rs"]
mod tests;
