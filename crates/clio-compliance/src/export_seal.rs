// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for sealing a readable export payload to one
//! recipient's X25519 public key, so no readable byte of a DSAR bundle
//! reaches disk.
//!
//! ## Owns
//! - `seal_to_recipient`: recipient text decoding, key derivation, and the
//!   `XChaCha20Poly1305` seal
//!
//! ## Does not own
//! - When a seal is required and which mode is readable (`crate::export`)
//! - Bundle payload construction (`crate::export_subject`)
//!
//! ## Boundary
//!
//! Changes to how a payload is sealed should normally require changes here.
//! Changes to what a readable payload contains should not.
//!
//! Keep this module focused on the recipient seal.
//!
//! ## Construction
//!
//! The recipient argument is standard base64 text of a 32-byte X25519 public
//! key. The seal draws a fresh ephemeral X25519 secret, agrees a shared secret
//! with the recipient public key, and derives a 32-byte key with HKDF-SHA256
//! over `salt = ephemeral_public || recipient_public` and
//! `info = b"clio-subject-export-seal-v1"`. `XChaCha20Poly1305` encrypts with a
//! fresh random 24-byte nonce and
//! `AAD = magic || ephemeral_public || recipient_public`.
//!
//! The file body is the magic `CLIO-SUBJECT-EXPORT-SEALED-V1`, the 32-byte
//! ephemeral public key, the 24-byte nonce, and the AEAD ciphertext. Only the
//! recipient's X25519 secret key opens it, and Clio keeps no part of that key.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use clio_types::{AmError, ErrorCode};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};

/// File magic for a sealed readable export.
pub const MAGIC: &[u8] = b"CLIO-SUBJECT-EXPORT-SEALED-V1";

/// HKDF `info` value that separates this key from every other use.
const KEY_INFO: &[u8] = b"clio-subject-export-seal-v1";

/// Length of an X25519 public key.
const PUBLIC_KEY_LEN: usize = 32;

/// Length of an `XChaCha20Poly1305` nonce.
const NONCE_LEN: usize = 24;

/// Seal `bytes` to the X25519 `recipient` and return the sealed file body.
///
/// The caller passes the exact recipient text the operator supplied. Malformed
/// recipient text is an argument error, so no destination is staged.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] when the recipient text is not standard
/// base64 of a 32-byte X25519 public key, or when the key is low order and the
/// agreed shared secret is all zero. RFC 7748 section 6.1 requires that abort.
pub(crate) fn seal_to_recipient(bytes: &[u8], recipient: &str) -> Result<Vec<u8>, AmError> {
    let recipient_public = decode_recipient(recipient)?;
    let ephemeral_secret: [u8; PUBLIC_KEY_LEN] = rand::random();
    let ephemeral_public = x25519(ephemeral_secret, X25519_BASEPOINT_BYTES);
    let shared = x25519(ephemeral_secret, recipient_public);
    if shared == [0u8; PUBLIC_KEY_LEN] {
        return Err(low_order_recipient());
    }

    let mut salt = [0u8; PUBLIC_KEY_LEN * 2];
    salt[..PUBLIC_KEY_LEN].copy_from_slice(&ephemeral_public);
    salt[PUBLIC_KEY_LEN..].copy_from_slice(&recipient_public);
    let key = derive_key(&shared, &salt);

    let mut aad = Vec::with_capacity(MAGIC.len() + PUBLIC_KEY_LEN * 2);
    aad.extend_from_slice(MAGIC);
    aad.extend_from_slice(&ephemeral_public);
    aad.extend_from_slice(&recipient_public);

    let nonce: [u8; NONCE_LEN] = rand::random();
    let cipher = XChaCha20Poly1305::new((&key).into());
    // XChaCha20Poly1305 encryption cannot fail for a valid key and nonce; the
    // only documented failure is a plaintext over 256 GiB.
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: bytes,
                aad: &aad,
            },
        )
        .expect("in-memory XChaCha20Poly1305 encryption");

    let mut sealed =
        Vec::with_capacity(MAGIC.len() + PUBLIC_KEY_LEN + NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(MAGIC);
    sealed.extend_from_slice(&ephemeral_public);
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ciphertext);
    Ok(sealed)
}

/// Decode standard base64 recipient text into a 32-byte X25519 public key.
fn decode_recipient(recipient: &str) -> Result<[u8; PUBLIC_KEY_LEN], AmError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(recipient.trim())
        .map_err(|_| invalid_recipient())?;
    <[u8; PUBLIC_KEY_LEN]>::try_from(decoded.as_slice()).map_err(|_| invalid_recipient())
}

/// One argument error for every shape of bad recipient text.
fn invalid_recipient() -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        "recipient must be standard base64 text of a 32-byte X25519 public key",
    )
}

/// A low-order recipient key agrees an all-zero shared secret, which is public.
fn low_order_recipient() -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        "recipient X25519 public key is low order: the shared secret is all zero",
    )
}

/// Derive the 32-byte seal key with HKDF-SHA256.
fn derive_key(shared: &[u8; PUBLIC_KEY_LEN], salt: &[u8]) -> [u8; PUBLIC_KEY_LEN] {
    let mut key = [0u8; PUBLIC_KEY_LEN];
    Hkdf::<Sha256>::new(Some(salt), shared)
        .expand(KEY_INFO, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

#[cfg(test)]
#[path = "export_seal_tests.rs"]
mod export_seal_tests;
