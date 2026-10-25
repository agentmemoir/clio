// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the optional client-side sync encryption
//! adapter (compatibility): sealing content with a local
//! sync key before push and opening it after pull. DEK material never leaves
//! the node; the sync server stays able to store ciphertext without keys.
//! The associated data is supplied by the caller — the canonical execution
//! envelope of the mutation being sealed — so ciphertext is bound to the
//! envelope it travels in and a relay cannot retarget it.
//!
//! ## Owns
//! - `SyncCipher` ciphertext container (alg, key id, nonce, base64 payload)
//! - `SyncCipherKey` local symmetric key handling, seal/open, and the wire key id
//! - The newest-key seal used by the client, and single-key open under the
//!   sealing role's subkey with no raw-key fallback
//!
//! ## Does not own
//! - Subject DEKs or KMS (see the `clio-store` DEK provider)
//! - Wire envelope shapes (see `protocol`)
//! - Server-side storage of opaque bytes (`server`)
//!
//! ## Boundary
//!
//! Changes to sync-key handling or the AEAD choice should normally require
//! changes here. Changes to the DEK/KMS interface should not require changes
//! here.
//!
//! Keep this module focused on the client-side sync encryption adapter.

use base64::Engine;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use clio_types::{AmError, ErrorCode};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// One client-encrypted content blob (opaque to the sync server).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncCipher {
    /// Algorithm tag (always `xchacha20poly1305` in this release).
    pub alg: String,
    /// Non-reversible identifier of the sealing key. Current writes always stamp
    /// one; a row without it is refused by the client rather than opened by
    /// trying held keys. `serde` ignores unknown fields on this struct and
    /// fills a missing field from `default`, so a reader deserializes rows
    /// written before the field existed and refuses them at open time, in both
    /// directions, without a protocol version bump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Base64 nonce.
    pub nonce_b64: String,
    /// Base64 ciphertext (includes Poly1305 tag).
    pub ciphertext_b64: String,
}

/// Local symmetric key used for outbound sync content (never transmitted).
#[derive(Clone)]
pub struct SyncCipherKey {
    /// 32-byte key held zeroized-on-drop.
    key: std::sync::Arc<Zeroizing<[u8; 32]>>,
}

impl std::fmt::Debug for SyncCipherKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SyncCipherKey(<masked>)")
    }
}

impl SyncCipherKey {
    /// Domain label for the wire key identifier, distinct from every other
    /// derivation in the workspace, including the status fingerprint's.
    const KEY_ID_DOMAIN: &[u8] = b"am-sync-key-id";

    /// Separator between the domain label and the key bytes, the same byte the
    /// workspace's other key derivations join with.
    const KEY_ID_SEP: u8 = 0x1f;

    /// Number of hex characters in a wire key identifier.
    const KEY_ID_CHARS: usize = 16;

    /// Derive a sync key from a profile-provided secret (never transmitted).
    #[must_use]
    pub fn from_secret(secret: &str) -> Self {
        let digest = clio_store::sha256_hex(secret.as_bytes(), "am-sync-key");
        let mut key = [0u8; 32];
        key.copy_from_slice(hex_to_bytes(&digest).as_slice());
        Self {
            key: std::sync::Arc::new(Zeroizing::new(key)),
        }
    }

    /// The key bytes, for the role derivations that take this key as their
    /// input. Reading them here is the only way another module can reach the
    /// key, and it cannot derive the key itself from anything on the wire.
    pub(crate) fn key_bytes(&self) -> &[u8] {
        self.key.as_ref().as_ref()
    }

    /// This key's sealing-role subkey. Sealing uses its own derived key rather
    /// than the sync key itself, so the content digest and the identifier
    /// handle — which use their own roles' subkeys — never share key material
    /// with the AEAD.
    pub(crate) fn seal_key(&self) -> Vec<u8> {
        crate::keyed_roles::role_subkey(self, crate::keyed_roles::ROLE_SEAL)
    }

    /// Open received ciphertext under the sealing role's derived key, matched
    /// by the stamped identifier before this call: a row no held key's
    /// identifier matches is refused upstream, never opened by trial. The
    /// digest role and the handle role derive their own subkeys, so nothing
    /// the AEAD touches shares key material with either of them.
    ///
    /// `aad` must be the associated data the writer sealed under, which is the
    /// execution envelope of the mutation as received.
    ///
    /// # Errors
    /// Unknown algorithm, bad base64, or authentication failure.
    pub fn open(&self, cipher: &SyncCipher, aad: &[u8]) -> Result<Vec<u8>, AmError> {
        Self::open_under(&self.seal_key(), cipher, aad)
    }

    /// The non-reversible wire identifier of this key: truncated,
    /// domain-separated hex over the key bytes. Two nodes holding the same key
    /// derive the same value; a node without the key cannot derive it from the
    /// identifier. The label differs from the status fingerprint's, so the wire
    /// value and the status value are never the same string, and an observer
    /// who sees both cannot turn either into the key.
    #[must_use]
    pub fn key_id(&self) -> String {
        truncated_hex_digest(
            Self::KEY_ID_DOMAIN,
            Self::KEY_ID_SEP,
            Self::KEY_ID_CHARS,
            self.key.as_ref().as_ref(),
        )
    }

    /// Seal plaintext content bytes under `aad`; returns the wire-safe
    /// ciphertext, stamped with this key's identifier so a receiving node can
    /// tell which of its keys opens it. The caller supplies the associated
    /// data, which authenticates the envelope the ciphertext travels in.
    ///
    /// # Errors
    /// AEAD failure (should not happen for this shape).
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<SyncCipher, AmError> {
        // The nonce is 24 raw bytes drawn fresh from the thread-local CSPRNG
        // on every call: reusing a nonce under one key is the failure this
        // prevents. `open` reads the nonce back from the wire, so nothing
        // else depends on where it came from.
        let nonce_bytes: [u8; 24] = rand::random();
        let cipher = XChaCha20Poly1305::new(self.seal_key().as_slice().into());
        // XChaCha20-Poly1305 encryption cannot fail for a valid key and nonce;
        // the only documented failure is a plaintext over 256 GiB.
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("sync cipher seal");
        Ok(SyncCipher {
            alg: "xchacha20poly1305".into(),
            key_id: Some(self.key_id()),
            nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
            ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(sealed),
        })
    }

    fn open_under(key: &[u8], cipher: &SyncCipher, aad: &[u8]) -> Result<Vec<u8>, AmError> {
        if cipher.alg != "xchacha20poly1305" {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("unsupported sync cipher: {}", cipher.alg),
            ));
        }
        let nonce = base64::engine::general_purpose::STANDARD
            .decode(&cipher.nonce_b64)
            .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "bad sync cipher nonce"))?;
        let body = base64::engine::general_purpose::STANDARD
            .decode(&cipher.ciphertext_b64)
            .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "bad sync cipher body"))?;
        let Ok(nonce_arr): Result<[u8; 24], _> = nonce.as_slice().try_into() else {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "sync cipher nonce must be 24 bytes",
            ));
        };
        let opener = XChaCha20Poly1305::new(key.into());
        let plaintext = opener
            .decrypt(XNonce::from_slice(&nonce_arr), Payload { msg: &body, aad })
            .map_err(|_| AmError::new(ErrorCode::Internal, "sync cipher open failed"))?;
        Ok(plaintext)
    }
}

/// Truncated, domain-separated hex digest of `key` bytes, the shape the
/// workspace's key fingerprints use.
fn truncated_hex_digest(domain: &[u8], sep: u8, chars: usize, key: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update([sep]);
    hasher.update(key);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(chars);
    for byte in &digest[..chars / 2] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0))
        .collect()
}

#[cfg(test)]
#[path = "crypto_nonce_tests.rs"]
mod crypto_nonce_tests;
#[cfg(test)]
#[path = "crypto_tests.rs"]
mod crypto_tests;
