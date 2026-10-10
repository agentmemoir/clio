// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one keyed-digest construction shared by every
//! caller that needs a value to be unpredictable to anyone who does not hold
//! the key: the lexical index's stored term, the sync content digest, and the
//! sync record-identifier handle. It is HMAC-SHA256 over a caller-supplied
//! domain label, truncated to a caller-requested width.
//!
//! ## Owns
//! - The HMAC-SHA256 construction (ipad/opad, the 64-byte block, the key
//!   length rule) and its truncation
//! - Lowercase-hex rendering of a truncated digest
//!
//! ## Does not own
//! - Token normalization and the stored term encoding (`lexical_digest`)
//! - Which domain label or width each role asks for (`clio-sync`)
//! - Key custody and key rotation (`clio-config`, `clio-sync`'s cipher)
//!
//! ## Boundary
//!
//! Changes to the construction itself should normally require changes here and
//! in every caller's expected digest. Changes to what a caller feeds it should
//! not require changes here.
//!
//! Keep this module focused on the keyed-digest construction.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// HMAC block size for SHA-256. A key no longer than this is used as is.
const HMAC_BLOCK: usize = 64;

/// Keyed digest of `message` under `key`, truncated to `out_bytes` and
/// rendered as lowercase hex.
///
/// The domain label is prepended inside the inner hash, so two callers sharing
/// a key but asking under different labels can never produce the same digest
/// for the same message. `out_bytes` is a parameter because the callers
/// disagree on purpose: the lexical index stores 16 bytes per term, and the
/// sync handle stores 16 bytes per record.
///
/// A key longer than the hash's block length is hashed down to the output
/// length first, which is what the construction requires; a shorter key is
/// zero-padded. Every key in this workspace is 32 bytes, so the shortening
/// branch never runs today, but a caller that passes a longer key gets the
/// specified behaviour rather than a silently truncated pad. `out_bytes` is
/// clamped to the hash output length, so a caller asking for more bytes than
/// the hash produces gets the whole digest rather than a slice panic.
#[must_use]
pub fn keyed_digest_hex(key: &[u8], domain: &[u8], message: &[u8], out_bytes: usize) -> String {
    let bytes = keyed_digest(key, domain, message, out_bytes);
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The same digest as `keyed_digest_hex`, as raw bytes.
#[must_use]
pub fn keyed_digest(key: &[u8], domain: &[u8], message: &[u8], out_bytes: usize) -> Vec<u8> {
    let shortened = shorten_key(key);
    let mut inner_pad = [0x36_u8; HMAC_BLOCK];
    let mut outer_pad = [0x5c_u8; HMAC_BLOCK];
    for (pad, byte) in inner_pad.iter_mut().zip(shortened.as_slice()) {
        *pad ^= byte;
    }
    for (pad, byte) in outer_pad.iter_mut().zip(shortened.as_slice()) {
        *pad ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(domain);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    let digest = outer.finalize();
    let take = out_bytes.min(digest.len());
    digest[..take].to_vec()
}

/// The key as the construction consumes it: a key longer than the block is
/// hashed down, anything shorter is returned as is.
fn shorten_key(key: &[u8]) -> Vec<u8> {
    if key.len() > HMAC_BLOCK {
        let mut hasher = Sha256::new();
        hasher.update(key);
        return hasher.finalize().to_vec();
    }
    key.to_vec()
}

#[cfg(test)]
#[path = "keyed_digest_tests.rs"]
mod tests;
