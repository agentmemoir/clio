// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the values the sync key lifecycle hands back:
//! one loaded key with its location, the non-reversible fingerprint, and the
//! report that names store, fingerprint, creation time, and key count.
//!
//! ## Owns
//! - [`LoadedSyncKey`], its zeroizing bytes, its base64 form, and its `Debug`
//! - [`SyncKeyReport`], the one shape every status surface renders
//! - The fingerprint derivation and the report assembly
//!
//! ## Does not own
//! - Generating, storing, reading, or deleting a key (see `sync_key`)
//! - The key file, its path, or its sidecar (see `sync_key_file`)
//! - The OS secret store (see `secret_key_store`)
//!
//! ## Boundary
//!
//! Changes to what a loaded key can disclose, or to the report's fields,
//! should normally require changes here. Changes to where a key lives should
//! not.
//!
//! Keep this module focused on the values and their derivations.

use std::fmt::Write as _;
use std::path::PathBuf;

use base64::Engine;
use clio_types::{AmError, ErrorCode};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::secret_key::KEY_LEN;
use crate::secret_key_source::KeySource;

/// The keystore attribute that holds the sync key's creation time.
///
/// A non-secret name the `keyring` backends either store (Linux) or ignore
/// without error (macOS and Windows). The caller always reads it back before
/// reporting it.
pub(crate) const CREATED_ATTRIBUTE: &str = "created_at";

/// Domain label for the fingerprint, distinct from every other derivation.
const FINGERPRINT_DOMAIN: &[u8] = b"am-sync-key-fingerprint";

/// Separator between the domain label and the key bytes, the same byte
/// `erase_hash` joins its salt and value with.
const FINGERPRINT_SEP: u8 = 0x1f;

/// Number of hex characters in a fingerprint.
const FINGERPRINT_CHARS: usize = 16;

/// Which store holds a loaded sync key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncKeyLocation {
    /// The owner-only sync key file at this path.
    File(PathBuf),
    /// The operating system's secret store, under the sync key's own account.
    Keystore,
}

impl std::fmt::Display for SyncKeyLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "key file {}", path.display()),
            Self::Keystore => f.write_str("the OS secret store"),
        }
    }
}

/// One loaded sync key plus where it came from.
///
/// The bytes have no `Debug` and no `Display`, so rendering this type can only
/// ever produce the location. [`Self::expose`], [`Self::base64`], and
/// [`Self::fingerprint`] are the only ways out.
pub struct LoadedSyncKey {
    bytes: Zeroizing<[u8; KEY_LEN]>,
    /// Which store the bytes came from.
    pub location: SyncKeyLocation,
}

impl LoadedSyncKey {
    pub(crate) fn new(bytes: Zeroizing<[u8; KEY_LEN]>, location: SyncKeyLocation) -> Self {
        Self { bytes, location }
    }

    /// Borrow the raw key bytes. The only way out of this type.
    #[must_use]
    pub fn expose(&self) -> &[u8; KEY_LEN] {
        &self.bytes
    }

    /// The key as standard base64, the one printed form this build uses.
    #[must_use]
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.expose().as_slice())
    }

    /// The non-reversible fingerprint: truncated, domain-separated hex.
    ///
    /// Stable for one key and different for another, so two nodes can compare
    /// keys by eye. It is derived on every call and never stored.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.bytes)
    }
}

impl std::fmt::Debug for LoadedSyncKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedSyncKey")
            .field("location", &self.location)
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// What `clio sync key status` reports, and what `sync_key_status` returns.
///
/// `fingerprint` and `created_at` are `None` when no key exists or when the
/// platform keeps no creation time; `created_at` is never invented. `keys`
/// lists every held key, the current one first and the archived ones after it
/// in creation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncKeyReport {
    /// The store the key is in, or the recorded one when no key exists yet.
    pub store: KeySource,
    /// The non-reversible fingerprint, when a key exists.
    pub fingerprint: Option<String>,
    /// The creation time, when the store retained it.
    pub created_at: Option<String>,
    /// How many keys are held: the current key plus every archived one.
    pub count: usize,
    /// Every held key, newest first: the current key, then the archived ones
    /// in creation order.
    pub keys: Vec<SyncKeyEntry>,
}

/// One held sync key's operator-facing facts. No field here ever carries key
/// bytes or a prefix of them: the fingerprint is derived and non-reversible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncKeyEntry {
    /// The non-reversible fingerprint of this key.
    pub fingerprint: String,
    /// The creation time, when the store retained it.
    pub created_at: Option<String>,
    /// Whether this entry is the current key, which seals new content.
    pub current: bool,
    /// Where this key is held: the recorded store's name for the current key,
    /// or the archive file for a superseded one.
    pub store: String,
}

pub(crate) fn generate_bytes() -> Zeroizing<[u8; KEY_LEN]> {
    Zeroizing::new(rand::random::<[u8; KEY_LEN]>())
}

/// The exact-length check for a key that came from the keystore.
pub(crate) fn key_from_bytes(
    raw: &Zeroizing<Vec<u8>>,
    at: &SyncKeyLocation,
) -> Result<Zeroizing<[u8; KEY_LEN]>, AmError> {
    if raw.len() != KEY_LEN {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the sync key in {at} is {} bytes; expected {KEY_LEN}. Clio will not replace it, \
                 because a new key cannot open anything sealed under the old one; restore the \
                 correct key, or remove the damaged one to start again with everything sealed \
                 under it lost",
                raw.len()
            ),
        ));
    }
    let mut key = Zeroizing::new([0_u8; KEY_LEN]);
    key.copy_from_slice(raw);
    Ok(key)
}

/// A report for a node that holds no sync key.
pub(crate) fn empty_report(store: KeySource) -> SyncKeyReport {
    SyncKeyReport {
        store,
        fingerprint: None,
        created_at: None,
        count: 0,
        keys: Vec::new(),
    }
}

/// A report for a loaded key. The archived keys, if any, are appended by the
/// caller, which also sets the final count.
pub(crate) fn report_of(
    key: &Zeroizing<[u8; KEY_LEN]>,
    store: KeySource,
    created_at: Option<String>,
) -> SyncKeyReport {
    let entry = SyncKeyEntry {
        fingerprint: fingerprint_of(key),
        created_at: created_at.clone(),
        current: true,
        store: store.as_str().to_owned(),
    };
    SyncKeyReport {
        store,
        fingerprint: Some(fingerprint_of(key)),
        created_at,
        keys: vec![entry],
        count: 1,
    }
}

/// The fingerprint of one archived key, held as standard base64.
///
/// # Errors
/// The value is not base64 or does not decode to a full key: a damaged archive
/// holds key material, so it is refused rather than guessed at.
pub(crate) fn fingerprint_of_base64(key_b64: &str) -> Result<String, AmError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .map_err(|_| damaged_archive_error(key_b64.len()))?;
    let Ok(raw): Result<[u8; KEY_LEN], _> = raw.as_slice().try_into() else {
        return Err(damaged_archive_error(key_b64.len()));
    };
    Ok(fingerprint_of(&Zeroizing::new(raw)))
}

/// The refusal for an archived key that is not a well-formed key.
fn damaged_archive_error(len: usize) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "an archived sync key record is {len} base64 characters, which is not a key this \
             build can read; Clio will not guess at key material, so restore the sync key \
             archive from backup"
        ),
    )
}

/// Truncated, domain-separated hex digest of the key bytes.
fn fingerprint_of(key: &[u8; KEY_LEN]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update([FINGERPRINT_SEP]);
    hasher.update(key);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(FINGERPRINT_CHARS);
    for byte in &digest[..FINGERPRINT_CHARS / 2] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[path = "sync_key_value_tests.rs"]
mod tests;
