// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the whole life of the one Clio encryption
//! key: finding it, creating it exactly once, and saying where it lives.
//!
//! ## Owns
//! - The rule set choosing between the OS secret store and the key file
//! - Generating, reading, and writing the 32 key bytes
//! - The keystore-wins rule when both hold a key, and the never-replace rule
//! - Reporting the store's location without creating a key
//!
//! ## Does not own
//! - Reading or writing the OS secret store itself (see `secret_key_store`)
//! - Sealing and opening values under the key (the sealing stage)
//! - Encrypting the key-store file (the sealing stage)
//!
//! ## Boundary
//!
//! Changes to key precedence, the key-file format, or the never-replace rule
//! should normally require changes here. Changes to the OS secret store's crate
//! or entry name should not require changes here.
//!
//! Keep this module focused on the key's lifecycle.
//!
//! # The rules, in the order they run
//!
//! 1. A key file exists. The keystore is asked whether it also holds a key, and
//!    if it does the keystore's key is used, because it is the store Clio
//!    prefers on a desktop and the one a headless machine cannot reach. The
//!    unused file is named on stderr so it can be deleted. A keystore that is
//!    empty, or that is unreachable and cannot answer, leaves the file in charge.
//! 2. No key file, and the keystore has an entry: use it and write no file.
//! 3. No key file, and the keystore is reachable but empty: generate, store in
//!    the keystore, and fall back to the file if the store will not take it.
//! 4. No key file and an unreachable keystore: generate into the file.
//!
//! Only the last two steps generate, and both are reached only when no key
//! exists anywhere. A key that exists but cannot be read is never replaced,
//! truncated, padded, or re-generated: every such failure needs an operator,
//! because a fresh key makes every value sealed under the old one unreadable.
//!
//! Errors are classified so a caller cannot mistake one of these for a
//! skippable content failure. A key that exists but is unusable is
//! [`ErrorCode::ConfigCorrupt`]; a store or filesystem that could not be
//! reached at all is [`ErrorCode::Internal`]. Neither is in the set that
//! tolerant read paths skip, so a two-store state or a locked keystore always
//! stops the caller.
//!
//! # What is not here
//!
//! Nothing loads the key eagerly. Config loading does not touch it, so a
//! command that never reads a secret never asks the OS for the key and never
//! fails because a Keychain is locked. Callers load it at the point of use.

use crate::secret_key_file::{self as file_store, key_file_path};
use crate::secret_key_source::{self, KeySource};
use crate::secret_key_store::{
    ACCOUNT, ENCRYPTION_KEY, KeyringStore, NoKeystore, SecretStore, display_name,
};
use clio_types::{AmError, ErrorCode};
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Length of the Clio encryption key in bytes.
pub const KEY_LEN: usize = 32;

/// Environment variable naming an explicit key-file path.
pub const KEY_FILE_ENV: &str = "CLIO_KEY_FILE";

/// Environment variable that names the key file as the only Clio key store.
///
/// Set this and Clio never asks the operating system's secret store anything.
/// The key file becomes the only place a key can be, and the two-store refusal
/// cannot fire because there is no second store to refuse.
///
/// **What this costs.** The two-store check exists because deleting either copy
/// later would silently change the key. Turning the check off means the operator
/// takes responsibility for that: if a key is also sitting in the OS keystore,
/// Clio will not notice, and the copy it does not know about is the one an
/// operator is most likely to delete first. Set this when you know the key file
/// is the only key — a headless machine, a container, CI, or a test run — and
/// leave it unset everywhere else.
///
/// The main reason it exists at all: the check is a Keychain read on macOS, and
/// the operating system may answer it with a password prompt. A test run that
/// starts hundreds of processes turns that into hundreds of prompts for no
/// benefit.
pub const KEY_FILE_ONLY_ENV: &str = "CLIO_KEY_FILE_ONLY";

/// Key-file name used under the resolved data directory.
pub const KEY_FILE_NAME: &str = "clio.key";

/// Where the key lives, for `clio status` and `clio doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyLocation {
    /// The operating system's secret store.
    Keystore,
    /// The owner-only key file at this path.
    File(PathBuf),
}

impl std::fmt::Display for KeyLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keystore => f.write_str(display_name()),
            Self::File(path) => write!(f, "key file {}", path.display()),
        }
    }
}

/// One loaded key plus where it came from.
///
/// The bytes have no `Debug` and no `Display`, so rendering this type can only
/// ever produce the location. `expose` is the single way out.
pub struct LoadedKey {
    bytes: Zeroizing<[u8; KEY_LEN]>,
    /// Which store the bytes came from.
    pub location: KeyLocation,
}

impl LoadedKey {
    fn new(bytes: Zeroizing<[u8; KEY_LEN]>, location: KeyLocation) -> Self {
        Self { bytes, location }
    }

    /// Borrow the raw key bytes. The only way out of this type.
    #[must_use]
    pub fn expose(&self) -> &[u8; KEY_LEN] {
        &self.bytes
    }
}

impl std::fmt::Debug for LoadedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedKey")
            .field("location", &self.location)
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// Load the existing key, creating one when no key exists anywhere.
///
/// # Errors
/// A key exists but cannot be used — wrong length, unreadable file, an
/// unreachable keystore with nowhere to fall back, or both stores populated.
/// Clio never replaces a key, so none of these are retryable: each one needs an
/// operator to fix the store.
///
/// # Panics
/// If the operating system's random source fails, which `rand::random` reports
/// by panicking rather than by returning.
pub fn load_or_create() -> Result<LoadedKey, AmError> {
    let store = process_store();
    load_or_create_with(&*store, key_file_path(&process_env))
}

/// Report where the key lives, creating nothing.
///
/// # Errors
/// The OS secret store could not be consulted. That is reported rather than
/// folded into `Ok(None)`, because "no key yet" and "cannot tell" are different
/// facts and a status line must not claim the first when the second is true.
///
/// This reports the store a load would use, so a line never names a key file
/// while the keystore holds the key a load would actually read.
pub fn location() -> Result<Option<KeyLocation>, AmError> {
    let store = process_store();
    location_with(&*store, key_file_path(&process_env).as_deref())
}

fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The keystore for this process, or [`NoKeystore`] when this process must not
/// read the operator's OS secrets.
///
/// A process that chose the file store does not read the keystore at all, because
/// asking it can cost a password prompt on macOS and a `NoKeystore` load is what
/// keeps a headless run from ever making that ask.
fn process_store() -> Box<dyn SecretStore> {
    if file_store::keystore_is_bypassed(&process_env) {
        return Box::new(NoKeystore);
    }
    Box::new(KeyringStore::os(ACCOUNT, ENCRYPTION_KEY))
}

/// Load the key from the store the operator chose, creating one when that store
/// is empty.
///
/// The choice comes from the marker, so there is nothing to reconcile: a machine
/// that chose the file store never reads the keystore, and one that chose the
/// keystore never reads the file. An absent marker means the file store, which is
/// the recommended one and the only one that works without an OS secret store.
fn load_or_create_with(
    store: &dyn SecretStore,
    file: Option<PathBuf>,
) -> Result<LoadedKey, AmError> {
    let source =
        secret_key_source::read(secret_key_source::marker_path(file.as_deref()).as_deref())?
            .unwrap_or_else(KeySource::recommended);
    match source {
        KeySource::File => load_from_file(file),
        KeySource::Keystore => load_from_keystore(store, file.as_deref()),
    }
}

/// The file store: read the key file, or create one.
///
/// A missing key file is created. A key in the keystore is not consulted and not
/// reported, because the operator chose this store and the other one is not a
/// second copy of anything Clio reads.
fn load_from_file(file: Option<PathBuf>) -> Result<LoadedKey, AmError> {
    let path = file.ok_or_else(|| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the Clio encryption key is kept in a file, but no key-file path could be \
                 resolved; set {KEY_FILE_ENV} to a writable path and run again"
            ),
        )
    })?;
    if path.exists() {
        let key = read_key_file(&path)?;
        return Ok(LoadedKey::new(key, KeyLocation::File(path)));
    }
    let key = generate();
    file_store::write_key_file(&path, &key)?;
    Ok(LoadedKey::new(key, KeyLocation::File(path)))
}

/// The keystore: read the key, or create one.
///
/// A key file that exists is reported and then ignored, because the operator chose
/// the keystore. Reporting rather than refusing is what lets an operator who has
/// both copies find the one Clio is not using.
fn load_from_keystore(store: &dyn SecretStore, file: Option<&Path>) -> Result<LoadedKey, AmError> {
    if let Some(path) = file.filter(|path| path.exists()) {
        report_shadowed_file(path);
    }
    match store.get() {
        Ok(Some(raw)) => Ok(LoadedKey::new(
            key_from_bytes(&Zeroizing::new(raw), &KeyLocation::Keystore)?,
            KeyLocation::Keystore,
        )),
        Ok(None) => generate_into_keystore(store),
        Err(err) => Err(AmError::new(
            err.code,
            format!(
                "the Clio encryption key is kept in {}, and it could not be reached: {}. Clio \
                 will not fall back to a key file, because that would move the key somewhere you \
                 did not choose; start the secret store and run again",
                display_name(),
                err.message
            ),
        )),
    }
}

/// Say once that a key file is being set aside, and how to remove it.
///
/// Printed rather than returned because the load succeeded: an operator reading
/// a message after a failed command never sees it, and this is the one fact they
/// need before a later run picks the other copy.
fn report_shadowed_file(path: &Path) {
    eprintln!(
        "Clio found a second encryption key, in the key file {}, and is using the one in {}. \
         Nothing already saved is affected. To remove the unused copy, delete {}.",
        path.display(),
        display_name(),
        path.display()
    );
}

/// No key file and a reachable, empty keystore: the one path that stores a new
/// key in the OS secret store.
/// A new key for the keystore.
///
/// A store that can be read but not written is refused rather than redirected to a
/// key file. The operator chose the keystore, and a key in a file they were not
/// told about is a key in a store they did not choose; the refusal says which
/// store failed instead.
fn generate_into_keystore(store: &dyn SecretStore) -> Result<LoadedKey, AmError> {
    let key = generate();
    store.set(key.as_slice()).map_err(|err| {
        AmError::new(
            err.code,
            format!(
                "the Clio encryption key is kept in {}, and a new key could not be stored there: \
                 {err}. Clio did not write it anywhere else, because you chose that store; make \
                 it writable and run again",
                display_name()
            ),
        )
    })?;
    Ok(LoadedKey::new(key, KeyLocation::Keystore))
}

/// The only place a key is generated.
///
/// `rand::random` is the same source the per-subject key code uses, so the
/// project has one random source rather than two.
fn generate() -> Zeroizing<[u8; KEY_LEN]> {
    Zeroizing::new(rand::random::<[u8; KEY_LEN]>())
}

fn read_key_file(path: &Path) -> Result<Zeroizing<[u8; KEY_LEN]>, AmError> {
    let location = KeyLocation::File(path.to_path_buf());
    let raw = Zeroizing::new(fs::read(path).map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot read the {location}: {err}. Clio will not generate a replacement, because a \
                 new key makes every sealed value unreadable; restore access to the file, or point \
                 {KEY_FILE_ENV} at the file that holds the key"
            ),
        )
    })?);
    key_from_bytes(&raw, &location)
}

/// The exact-length check, shared by both stores.
///
/// A readable key of any other length is refused, never trimmed, padded, or
/// replaced. A wrong length means these bytes are not the key, and guessing
/// would lock away everything sealed under the real one.
fn key_from_bytes(
    raw: &Zeroizing<Vec<u8>>,
    at: &KeyLocation,
) -> Result<Zeroizing<[u8; KEY_LEN]>, AmError> {
    if raw.len() != KEY_LEN {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the key in {at} is {} bytes; expected {KEY_LEN}. Clio will not replace it, because \
                 a new key makes every sealed value unreadable; restore the correct key, or remove \
                 the damaged one to start again with everything sealed under it lost",
                raw.len()
            ),
        ));
    }
    let mut key = Zeroizing::new([0_u8; KEY_LEN]);
    key.copy_from_slice(raw);
    Ok(key)
}

/// Where a load would find the key, without creating one.
///
/// The order matches [`load_or_create`]: the keystore is asked first, and a key
/// file only answers when the keystore has nothing. Reporting the file while the
/// keystore holds the key that a load would use would name the wrong store in
/// `clio status` and in the applied summary.
fn location_with(
    store: &dyn SecretStore,
    file: Option<&Path>,
) -> Result<Option<KeyLocation>, AmError> {
    let source = secret_key_source::read(secret_key_source::marker_path(file).as_deref())?
        .unwrap_or_else(KeySource::recommended);
    match source {
        // Read through the `Zeroizing` so a key seen here is scrubbed on the way
        // out rather than dropped unread. `location` reports where a key is,
        // never what it is, so the bytes themselves are discarded.
        KeySource::Keystore => Ok(store.get()?.map(|_| KeyLocation::Keystore)),
        KeySource::File => Ok(file
            .filter(|path| path.exists())
            .map(|path| KeyLocation::File(path.to_path_buf()))),
    }
}

#[cfg(test)]
#[path = "secret_key_report_tests.rs"]
mod report_tests;

#[cfg(test)]
#[path = "secret_key_support_tests.rs"]
mod support_tests;

#[cfg(test)]
#[path = "secret_key_tests.rs"]
mod tests;

/// The never-replace rule, in its own module.
///
/// Split out because `secret_key_tests.rs` reached the workspace's file-size
/// limit, and the key file's integrity is a different question from which store
/// a first run picks.
#[cfg(test)]
#[path = "secret_key_integrity_tests.rs"]
mod integrity_tests;
