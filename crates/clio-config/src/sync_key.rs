// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the whole life of the Clio sync key: creating
//! it with the operator's chosen store, reading it back, deleting it, and
//! reporting store, fingerprint, creation time, and key count.
//!
//! ## Owns
//! - The recorded store choice (`sync-key-source`) and reading it back
//! - The env-driven lifecycle: generate (with rotation), load, delete, and
//!   report against either store
//! - Which store answers for the current key in this process
//!
//! ## Does not own
//! - The store-injected lifecycle variants (see `sync_key_store`)
//! - Superseded-key retention, the archive file, and the newest-first key
//!   list (see `sync_key_rotation`)
//! - The loaded key, the fingerprint, and the report shape (see `sync_key_value`)
//! - The sync key file, its path, or its sidecar (see `sync_key_file`)
//! - The OS secret store itself (see `secret_key_store`)
//! - Turning a supplied string into a sealing key (see `clio-sync`'s
//!   `SyncCipherKey::from_secret`, which this module never calls)
//!
//! ## Boundary
//!
//! Changes to the sync key's lifecycle or its store choice should normally
//! require changes here. Changes to how a key is used to seal content should
//! not.
//!
//! Keep this module focused on the sync key's lifecycle.
//!
//! # Why this mirrors the encryption key instead of wrapping it
//!
//! The two keys are separate values with different lifetimes and different blast
//! radii, so a wrapper would imply a relationship that does not exist; two small
//! modules that follow the same rules say what is true.
//!
//! # Why generation rotates instead of replacing
//!
//! A second generation that replaced the first would make everything sealed
//! under the old key unreadable with no error anywhere. Generation therefore
//! archives the superseded key first (see `sync_key_rotation`), so content
//! sealed under it stays readable and the store keeps meaning "the current
//! key".

use std::path::{Path, PathBuf};

use base64::Engine;
use clio_types::{AmError, ErrorCode};

use crate::secret_key_source::{self, KeySource};
use crate::secret_key_store::{KeyringStore, NoKeystore, SYNC_ACCOUNT, SYNC_KEY, SecretStore};
use crate::sync_key_file as file_store;
use crate::sync_key_rotation as rotation;

pub use crate::sync_key_file::{SYNC_KEY_FILE_ENV, SYNC_KEY_FILE_NAME, key_file_path};
pub use crate::sync_key_rotation::{load_all_base64, load_archive_base64};
pub use crate::sync_key_value::{LoadedSyncKey, SyncKeyEntry, SyncKeyLocation, SyncKeyReport};

/// The store the operator recorded for the sync key, or `None`.
///
/// # Errors
/// The marker exists but cannot be read, or names a store Clio does not know.
///
/// An unreadable marker is reported rather than read as no choice, because the
/// caller here is the wizard, which is about to ask the same question and would
/// then fail to record the answer. A machine that answered once must not be
/// asked again because its record went bad.
pub fn recorded_source<F>(get: &F) -> Result<Option<KeySource>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    source_or_error(get)
}

/// The recorded store, with an unreadable or unrecognised marker reported.
///
/// The marker exists, so the operator made a choice, and guessing a different
/// store than the one they chose is the one answer that cannot be walked back.
/// Every path that acts on the store uses this rather than
/// [`recorded_source`].
fn source_or_error<F>(get: &F) -> Result<Option<KeySource>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    secret_key_source::read(secret_key_source::sync_marker_path(file.as_deref()).as_deref())
}

/// Record the store choice, or confirm the one already recorded.
///
/// # Errors
/// A marker exists that names a store Clio does not know, or the marker cannot
/// be written. Both leave the choice unmade rather than made wrongly.
pub fn record_source<F>(get: &F, source: KeySource) -> Result<KeySource, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    record_at(&marker_destination(get)?, source)
}

/// Where the store choice is recorded, with a refusal when it has nowhere to go.
///
/// Split out of [`record_source`] so [`generate`] can settle it before its first
/// write rather than in the middle of one.
fn marker_destination<F>(get: &F) -> Result<PathBuf, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    secret_key_source::sync_marker_path(file.as_deref()).ok_or_else(|| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the sync key store choice has nowhere to be recorded; set {SYNC_KEY_FILE_ENV} to \
                 a writable path and run again"
            ),
        )
    })
}

/// Confirm the choice at `marker`, or write it when none is there.
fn record_at(marker: &Path, source: KeySource) -> Result<KeySource, AmError> {
    if let Some(recorded) = secret_key_source::read(Some(marker))? {
        return Ok(recorded);
    }
    secret_key_source::write(Some(marker), source)?;
    Ok(source)
}

/// Load the stored sync key, if one exists.
///
/// The recorded store decides where to look; no marker means the file store,
/// which is the recommended one. A keystore that cannot be reached is an error,
/// never a silent "no key".
///
/// # Errors
/// The key exists but cannot be used: wrong length, unreadable file, wrong
/// permissions, or an unreachable keystore.
pub fn load<F>(get: &F) -> Result<Option<LoadedSyncKey>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    let source = source_or_error(get)?.unwrap_or_else(KeySource::recommended);
    load_with(&*process_store(source), source, file)
}

/// Load the recorded sync key as standard base64, if one exists.
///
/// This is the form the MCP custody read gathers, so the file store and the
/// keystore both answer in the same shape.
///
/// # Errors
/// The same failures [`load`] reports.
pub fn load_base64<F>(get: &F) -> Result<Option<String>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    Ok(load(get)?.map(|key| key.base64()))
}

/// Read one sync key file as base64, with the owner-only and length checks.
///
/// The MCP custody helper calls this for the explicit file named by
/// `CLIO_SYNC_KEY_FILE`, which is the newest key and outranks the recorded
/// store.
///
/// # Errors
/// The file is missing, not owner-only, unreadable, or not
/// [`KEY_LEN`](crate::secret_key::KEY_LEN) bytes.
pub fn read_key_file_base64(path: &Path) -> Result<String, AmError> {
    let key = file_store::read_key(path)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(key.as_slice()))
}

/// Generate and store a new sync key, rotating any key already held.
///
/// `requested` is the `--store` flag. A recorded choice that disagrees with it
/// is refused rather than overridden: the marker is what loads read, and a key
/// written to the other store would never be found.
///
/// A key that already exists is archived, with the creation time the store
/// retained, before the new key is written: content sealed under it stays
/// readable, because the client holds the archived key and opens with any key
/// it holds. The archive entry is on disk before the old key leaves its
/// store, so an interrupted rotation loses nothing.
///
/// Nothing is written until every precondition holds: the store choice has
/// somewhere to go, and the superseded key has been archived. Recording before
/// the key byte keeps a marker failure from stranding a key; refusing before
/// the record keeps a refused run from naming a store no key ever reached,
/// which is what makes the advice to remove that record work.
///
/// # Errors
/// A shadowing key file sits beside a keystore key, the recorded choice
/// disagrees with `requested`, the current key or the archive cannot be
/// written, the store choice cannot be recorded, no file path resolves for the
/// file store, or the store refused the write.
pub fn generate<F>(get: &F, requested: Option<KeySource>) -> Result<LoadedSyncKey, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    let recorded = source_or_error(get)?;
    let source = match (requested, recorded) {
        (Some(wanted), Some(held)) if wanted != held => {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "the sync key store is already recorded as `{held}`, so a key was not written \
                     to `{wanted}`; run `clio sync key export` to read the key, or remove the \
                     sync-key-source record to choose a different store"
                ),
            ));
        }
        (Some(wanted), _) => wanted,
        (None, Some(held)) => held,
        (None, None) => KeySource::recommended(),
    };
    let marker = marker_destination(get)?;
    let store = process_store(source);
    // The one custody shape rotation does not take on is refused before the
    // record is confirmed, so a refused run still leaves the machine exactly
    // as it found it.
    rotation::refuse_shadowing_custody(source, file.as_deref())?;
    // The record is confirmed before anything moves: a marker that cannot be
    // written leaves the machine exactly as it found it, which is what makes
    // the advice to fix the record safe to follow.
    record_at(&marker, source)?;
    // The superseded key is archived and let go of before anything new is
    // written, so `generate_with`'s own refusal finds nothing in the way and
    // content sealed under the old key stays readable.
    rotation::rotate_existing(&*store, source, file.as_deref())?;
    generate_with(&*store, source, file)
}

/// Delete the stored sync key and its file-store metadata.
///
/// The recorded store choice stays, so a later `keygen` writes to the same
/// place. Deleting a key that is not there succeeds.
///
/// # Errors
/// The store or the filesystem refused the removal.
pub fn delete<F>(get: &F) -> Result<(), AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    let source = source_or_error(get)?.unwrap_or_else(KeySource::recommended);
    delete_with(&*process_store(source), source, file)
}

/// Report store, fingerprint, creation time, and key count without exposing a key.
///
/// # Errors
/// The recorded store is unreachable, or an existing key file is damaged.
pub fn report<F>(get: &F) -> Result<SyncKeyReport, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = file_store::key_file_path(get);
    let source = source_or_error(get)?.unwrap_or_else(KeySource::recommended);
    report_with(&*process_store(source), source, file)
}

pub use crate::sync_key_store::{delete_with, generate_with, load_with, report_with};

/// The production store for one source: the OS store, or nothing to ask.
fn process_store(source: KeySource) -> Box<dyn SecretStore> {
    if source == KeySource::File {
        return Box::new(NoKeystore);
    }
    Box::new(KeyringStore::os(SYNC_ACCOUNT, SYNC_KEY))
}

#[cfg(test)]
#[path = "sync_key_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_key_marker_tests.rs"]
mod marker_tests;

#[cfg(test)]
#[path = "sync_key_env_tests.rs"]
mod env_tests;
