// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-subject DEK provider contract and its
//! local development implementation, keeping KMS material outside the item store.
//!
//! ## Owns
//! - The `DekProvider` trait (ensure / encrypt / decrypt / destroy / diagnose)
//! - `CiphertextEnvelope`, the versioned ciphertext shape stored in item rows
//! - `LocalDevKms`, in-memory or file-backed
//!
//! ## Does not own
//! - The slot model and its store operations (see `dek_slots`)
//! - The on-disk key-store format and rows (see `local_kms_db`)
//! - Choosing the key-store path (see `clio-config`'s `kms_path`)
//! - Which item columns are encrypted (see the store implementations)
//!
//! ## Boundary
//!
//! Changes to DEK lifecycle or the envelope shape should normally require
//! changes here. Changes to key-store storage or slot operations should not
//! require changes here.
//!
//! Keep this module focused on the DEK provider and its crypto.

use crate::dek_slots::{KeySlot, SlotStore};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use clio_types::{AmError, ErrorCode};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

const NONCE_LEN: usize = 12;

/// Reserved key-store subject id for the store's lexical blind-index key.
///
/// It names a key slot, not a data subject: no item content is ever sealed
/// under it, and an erasure request never reaches it. It is deliberately not a
/// valid-looking subject id so an operator can see it is infrastructure.
pub(crate) const BLIND_INDEX_KEY_SUBJECT: &str = "__clio_lexical_blind_index_v1";

/// Versioned ciphertext envelope stored in `items.content_ciphertext`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiphertextEnvelope {
    /// Data-subject id this ciphertext was sealed under.
    pub subject_id: String,
    /// DEK version used at seal time.
    pub key_version: u32,
    /// Base64 nonce.
    pub nonce_b64: String,
    /// Base64 ciphertext (includes Poly1305 tag).
    pub ciphertext_b64: String,
}

impl CiphertextEnvelope {
    /// Encode envelope as JSON text for the content column.
    ///
    /// # Errors
    /// Serialization failure (should not happen for this shape).
    pub fn to_storage_string(&self) -> String {
        serde_json::to_string(self).expect("CiphertextEnvelope always serializes")
    }

    /// Decode envelope JSON from the content column.
    ///
    /// # Errors
    /// Malformed envelope JSON. Reported as
    /// [`clio_types::ErrorCode::UnreadableContent`]: the row's own content is
    /// unreadable, which is not a backend failure.
    pub fn from_storage_string(raw: &str) -> Result<Self, AmError> {
        serde_json::from_str(raw).map_err(|e| {
            AmError::new(
                ErrorCode::UnreadableContent,
                format!("ciphertext envelope decode failed: {e}"),
            )
        })
    }
}

/// Key-management boundary for content encryption.
pub trait DekProvider: Send + Sync {
    /// Ensure an active DEK exists; returns an opaque `dek_ref` (never raw key bytes).
    ///
    /// # Errors
    /// Provider failures.
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError>;

    /// The store's 32-byte key for the lexical blind index, without creating it.
    ///
    /// The lexical index stores one HMAC-SHA256 digest per stemmed term under
    /// this key, and the query path recomputes the same digests from the query
    /// terms, so every process that searches one store must resolve the same
    /// bytes. A query spans many data subjects, so the key is store-scoped,
    /// never per-subject: a per-subject key would leave the query unable to
    /// derive the digests it has to match.
    ///
    /// This is the read-path half: it never creates key material. When the key
    /// store is absent, or holds no blind-index slot yet, it fails with
    /// [`clio_types::ErrorCode::ConfigCorrupt`] instead of returning an empty
    /// result set, so a search against a keyless or not-yet-keyed store is
    /// loud rather than silently empty. The write path fails closed on it too:
    /// a gated write's dedup scan reaches the index through this same lookup,
    /// so a missing key store surfaces there as `factor_unavailable` — the
    /// write is refused rather than silently rekeying or degrading the
    /// neighbor scan to empty.
    ///
    /// Implementations persist the key in the key store — never beside the
    /// index it protects, never in a log, and never rendered as text. The key
    /// is stable for the life of the store; replacing it makes existing index
    /// rows unmatchable until a lexical reindex rebuilds them.
    ///
    /// # Errors
    /// The key store has no usable blind-index key, or cannot be read.
    fn blind_index_key(&self) -> Result<[u8; 32], AmError>;

    /// The store's blind-index key, creating and persisting it when missing.
    ///
    /// This is the write-path half, for callers that already expect to create
    /// key material (the lexical document write). Read paths must use
    /// [`DekProvider::blind_index_key`], which never creates anything: a read
    /// that created a key store would leave a file holding a key that cannot
    /// open the store, and a backup would then claim to carry usable key
    /// material while it cannot decrypt the payload.
    ///
    /// # Errors
    /// Provider failures (an unreadable or unwritable key store, for example).
    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError>;

    /// Seal plaintext under the subject's active DEK.
    ///
    /// # Errors
    /// Missing/destroyed key or AEAD failure.
    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError>;

    /// Open a ciphertext envelope.
    ///
    /// # Errors
    /// Destroyed key, version mismatch, or AEAD failure.
    ///
    /// Implementations must keep the two kinds of failure apart, because callers
    /// that read one optional row (an admission neighbor scan, a tolerant
    /// listing) skip content failures and still fail closed on backend ones:
    ///
    /// - **This envelope is unreadable** — malformed fields, bad base64, a nonce
    ///   of the wrong length, or failed authentication. Report a code for which
    ///   [`clio_types::AmError::is_unreadable_content`] returns `true`, as
    ///   `LocalDevKms` does below.
    /// - **The key store is unavailable** — an unreadable, locked, or corrupt key
    ///   backend. Report [`clio_types::ErrorCode::Internal`], which that
    ///   predicate deliberately excludes.
    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError>;

    /// Destroy the subject's DEK (crypto-shred). Irreversible for that subject.
    ///
    /// # Errors
    /// Provider failures.
    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError>;

    /// Diagnostic view with DEK material masked.
    fn diagnose_masked(&self, subject_id: &str) -> String;
}

/// Local-dev KMS. Keys never enter the item database.
///
/// [`LocalDevKms::new`] is process-local (the default for in-memory databases
/// and tests). [`LocalDevKms::open_persistent`] keeps the same keys across
/// processes by backing the slots with a file, so a one-shot CLI `remember`
/// and a later `recall` process share the same DEKs.
pub struct LocalDevKms {
    state: Mutex<SlotStore>,
}

impl Default for LocalDevKms {
    /// The default is an empty, process-local local KMS.
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LocalDevKms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalDevKms { slots: <redacted> }")
    }
}

impl LocalDevKms {
    /// Create an empty, process-local local KMS.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SlotStore::in_memory().expect("open in-memory local KMS")),
        }
    }

    /// Open a file-backed local KMS, loading any keys already at `path`.
    ///
    /// # Errors
    /// The store exists but cannot be opened, migrated, or has an unsupported
    /// version; this fails closed rather than silently discarding keys.
    pub fn open_persistent(path: impl Into<PathBuf>) -> Result<Self, AmError> {
        Ok(Self {
            state: Mutex::new(SlotStore::open(path.into())?),
        })
    }

    fn dek_ref(subject_id: &str, version: u32) -> String {
        format!("local:{subject_id}:v{version}")
    }
}

/// The read-path error for a key store with no usable blind-index slot.
///
/// A missing slot is not "no results": the query cannot derive the digests it
/// has to match, so the search must fail loudly instead of silently returning
/// an empty page. The usual causes are a deleted or wrong key store, or index
/// rows written before the keyed form that have not been reindexed yet.
fn blind_index_key_missing() -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        "the key store for this database holds no lexical blind-index key, so the \
         lexical index cannot be searched; restore the correct key store, then run \
         `clio ops reindex --target lexical` if the index rows are stale",
    )
}

impl DekProvider for LocalDevKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        if subject_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "subject_id must be non-empty",
            ));
        }
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        let (version, _) = store.ensure(subject_id)?;
        Ok(Self::dek_ref(subject_id, version))
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        match store.lookup(BLIND_INDEX_KEY_SUBJECT)? {
            Some(KeySlot::Active { key, .. }) => Ok(key.bytes),
            Some(KeySlot::Destroyed { .. }) => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "the lexical blind-index key has been destroyed; restore the key store",
            )),
            None => Err(blind_index_key_missing()),
        }
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        let (_, key) = store.ensure(BLIND_INDEX_KEY_SUBJECT)?;
        Ok(key)
    }

    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        let (version, key_bytes) = store.ensure(subject_id)?;
        let cipher = ChaCha20Poly1305::new((&key_bytes).into());
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .expect("ChaCha20Poly1305 encrypt with valid key/nonce");
        Ok(CiphertextEnvelope {
            subject_id: subject_id.to_owned(),
            key_version: version,
            nonce_b64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                nonce_bytes,
            ),
            ciphertext_b64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                ciphertext,
            ),
        })
    }

    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        let slot = store.lookup(&envelope.subject_id)?;
        let (key, version) = match slot {
            None => {
                return Err(AmError::new(
                    ErrorCode::Forbidden,
                    format!("no DEK for subject `{}`", envelope.subject_id),
                ));
            }
            Some(KeySlot::Destroyed { .. }) => {
                return Err(AmError::new(
                    ErrorCode::ErasedSubject,
                    format!(
                        "DEK for subject `{}` has been destroyed",
                        envelope.subject_id
                    ),
                ));
            }
            Some(KeySlot::Active { version, key }) => (key, version),
        };
        if version != envelope.key_version {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                "DEK version mismatch for ciphertext envelope",
            ));
        }
        let nonce_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &envelope.nonce_b64,
        )
        .map_err(|_| AmError::new(ErrorCode::UnreadableContent, "invalid nonce encoding"))?;
        if nonce_bytes.len() != NONCE_LEN {
            return Err(AmError::new(
                ErrorCode::UnreadableContent,
                "invalid nonce length",
            ));
        }
        let ct = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &envelope.ciphertext_b64,
        )
        .map_err(|_| AmError::new(ErrorCode::UnreadableContent, "invalid ciphertext encoding"))?;
        let cipher = ChaCha20Poly1305::new((&key.bytes).into());
        let nonce = Nonce::from_slice(&nonce_bytes);
        cipher
            .decrypt(nonce, ct.as_ref())
            .map_err(|_| AmError::new(ErrorCode::Forbidden, "content decryption failed"))
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        store.destroy(subject_id)
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        let mut store = self.state.lock().expect("LocalDevKms mutex poisoned");
        match store.lookup(subject_id) {
            Ok(Some(KeySlot::Active { version, .. })) => {
                format!(
                    "subject_id={subject_id} status=active dek_ref={}",
                    clio_config::secret::mask_secret(&Self::dek_ref(subject_id, version))
                )
            }
            Ok(Some(KeySlot::Destroyed { version })) => {
                format!(
                    "subject_id={subject_id} status=shredded version={version} dek_ref=[REDACTED]"
                )
            }
            Ok(None) => format!("subject_id={subject_id} status=absent"),
            Err(_) => format!("subject_id={subject_id} status=unavailable"),
        }
    }
}

#[cfg(test)]
#[path = "dek_tests.rs"]
mod tests;
