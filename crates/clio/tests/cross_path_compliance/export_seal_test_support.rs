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

/// File magic the readable export seal writes.
const EXPORT_SEAL_MAGIC: &[u8] = b"CLIO-SUBJECT-EXPORT-SEALED-V1";

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
pub(crate) fn open_export_seal(sealed: &[u8]) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};

    let body = &sealed[EXPORT_SEAL_MAGIC.len()..];
    let mut ephemeral = [0u8; 32];
    ephemeral.copy_from_slice(&body[..32]);
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&body[32..56]);

    let recipient = export_seal_public();
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(&ephemeral);
    salt[32..].copy_from_slice(&recipient);
    let shared = x25519_dalek::x25519(export_seal_secret(), ephemeral);
    let mut key = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &shared)
        .expand(b"clio-subject-export-seal-v1", &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");

    let mut aad = Vec::new();
    aad.extend_from_slice(EXPORT_SEAL_MAGIC);
    aad.extend_from_slice(&ephemeral);
    aad.extend_from_slice(&recipient);

    XChaCha20Poly1305::new((&key).into())
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &body[56..],
                aad: &aad,
            },
        )
        .expect("open the sealed bundle")
}
