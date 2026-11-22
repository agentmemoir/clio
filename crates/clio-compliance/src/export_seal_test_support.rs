// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared readable-export test identity: one
//! X25519 key pair per test process, and the helpers that open a sealed bundle.
//!
//! ## Owns
//! - `test_secret`, `test_recipient`, `open_sealed`, and `read_sealed`
//!
//! ## Does not own
//! - Export fixtures and the bundle copy helper (see `export_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::path::Path;

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};

use crate::export_seal::MAGIC;

/// The one X25519 secret every readable test export is sealed to.
///
/// The bytes are fixed so a test never depends on the host random source; the
/// key protects test fixtures and nothing else.
pub(crate) fn test_secret() -> [u8; 32] {
    [
        0x5a, 0x11, 0x9c, 0x2f, 0x64, 0xd3, 0x08, 0xb7, 0x21, 0x8e, 0x4a, 0x36, 0xf0, 0x9d, 0x55,
        0xc2, 0x73, 0x0b, 0xe8, 0x47, 0x1a, 0x96, 0x3c, 0xdf, 0x82, 0x6b, 0x04, 0xb9, 0xe5, 0x30,
        0x7c, 0xa1,
    ]
}

/// The X25519 public key matching [`test_secret`], as base64 recipient text.
pub(crate) fn test_recipient() -> String {
    let public = x25519(test_secret(), X25519_BASEPOINT_BYTES);
    base64::engine::general_purpose::STANDARD.encode(public)
}

/// Open a sealed export body with [`test_secret`].
///
/// Returns the plaintext, or an error message when the body is malformed or the
/// AEAD tag does not verify.
pub(crate) fn open_sealed(sealed: &[u8]) -> Result<Vec<u8>, String> {
    let Some(body) = sealed.strip_prefix(MAGIC) else {
        return Err("missing sealed-export magic".to_owned());
    };
    if body.len() < 32 + 24 {
        return Err("sealed export body is too short".to_owned());
    }
    let mut ephemeral = [0u8; 32];
    ephemeral.copy_from_slice(&body[..32]);
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&body[32..56]);
    let ciphertext = &body[56..];

    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(&ephemeral);
    salt[32..].copy_from_slice(&x25519(test_secret(), X25519_BASEPOINT_BYTES));
    let mut key = [0u8; 32];
    let shared = x25519(test_secret(), ephemeral);
    Hkdf::<Sha256>::new(Some(&salt), &shared)
        .expand(b"clio-subject-export-seal-v1", &mut key)
        .map_err(|_| "sealed export key length".to_owned())?;

    let mut aad = Vec::with_capacity(MAGIC.len() + 64);
    aad.extend_from_slice(MAGIC);
    aad.extend_from_slice(&ephemeral);
    aad.extend_from_slice(&salt[32..]);

    XChaCha20Poly1305::new((&key).into())
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| "sealed export did not open".to_owned())
}

/// Read an exported file as text, opening a sealed readable bundle.
pub(crate) fn read_sealed(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read the exported file");
    let bytes = if bytes.starts_with(MAGIC) {
        open_sealed(&bytes).expect("open the sealed export")
    } else {
        bytes
    };
    String::from_utf8(bytes).expect("export text is UTF-8")
}
