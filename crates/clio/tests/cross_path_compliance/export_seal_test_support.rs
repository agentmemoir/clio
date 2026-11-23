// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the readable-export seal helpers the
//! cross-path scenarios need: one fixed X25519 test identity, its base64
//! recipient text, and the open helper for a delivered bundle.
//!
//! ## Owns
//! - `export_recipient` and `open_export_seal`
//!
//! ## Does not own
//! - The seal itself (see the compliance export module)
//! - Scenario assertions (see the scenario modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

// File magic the readable export seal writes, from the seal's own crate.
use clio_compliance::READABLE_EXPORT_SEAL_MAGIC as EXPORT_SEAL_MAGIC;

/// Length of an X25519 public key in the sealed header.
const EPHEMERAL_LEN: usize = 32;

/// Length of the `XChaCha20Poly1305` nonce in the sealed header.
const NONCE_LEN: usize = 24;

/// One fixed X25519 secret for this scenario's readable export.
pub(crate) fn export_seal_secret() -> [u8; 32] {
    [0x3c; 32]
}

/// The base64 recipient text matching [`export_seal_secret`].
pub(crate) fn export_recipient() -> String {
    use base64::Engine as _;

    base64::engine::general_purpose::STANDARD.encode(export_seal_public())
}

/// The X25519 public key matching [`export_seal_secret`].
pub(crate) fn export_seal_public() -> [u8; 32] {
    x25519_dalek::x25519(export_seal_secret(), x25519_dalek::X25519_BASEPOINT_BYTES)
}

/// Open a sealed readable export with [`export_seal_secret`].
///
/// A body without the seal magic, or one too short to hold its header, returns
/// a clear error instead of panicking on a slice.
pub(crate) fn open_export_seal(sealed: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::aead::{Aead, Key, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};

    let Some(body) = sealed.strip_prefix(EXPORT_SEAL_MAGIC) else {
        return Err("missing sealed-export magic".to_owned());
    };
    if body.len() < EPHEMERAL_LEN + NONCE_LEN {
        return Err("sealed export body is too short".to_owned());
    }
    let mut ephemeral = [0u8; EPHEMERAL_LEN];
    ephemeral.copy_from_slice(&body[..EPHEMERAL_LEN]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&body[EPHEMERAL_LEN..EPHEMERAL_LEN + NONCE_LEN]);

    let recipient = export_seal_public();
    let mut salt = [0u8; EPHEMERAL_LEN * 2];
    salt[..EPHEMERAL_LEN].copy_from_slice(&ephemeral);
    salt[EPHEMERAL_LEN..].copy_from_slice(&recipient);
    let shared = x25519_dalek::x25519(export_seal_secret(), ephemeral);
    let mut key = Key::<XChaCha20Poly1305>::default();
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &shared)
        .expand(b"clio-subject-export-seal-v1", &mut key)
        .map_err(|_| "sealed export key length".to_owned())?;

    let mut aad = Vec::new();
    aad.extend_from_slice(EXPORT_SEAL_MAGIC);
    aad.extend_from_slice(&ephemeral);
    aad.extend_from_slice(&recipient);

    XChaCha20Poly1305::new(&key)
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &body[EPHEMERAL_LEN + NONCE_LEN..],
                aad: &aad,
            },
        )
        .map_err(|_| "sealed export did not open".to_owned())
}
