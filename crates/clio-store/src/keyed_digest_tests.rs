// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the shared keyed-digest
//! construction in `keyed_digest`: its determinism, its sensitivity to the key,
//! the domain label, the message and the requested width, and its agreement
//! with the HMAC-SHA256 vectors the construction is specified against.
//!
//! ## Owns
//! - The tests for `keyed_digest_hex` and `keyed_digest`
//!
//! ## Does not own
//! - The lexical index's own encoding and prefix (`lexical_digest_tests`)
//! - Sync role labels and widths (`clio-sync`'s tests)
//!
//! ## Boundary
//!
//! Changes to the keyed-digest construction should normally require changes
//! here. Changes to one caller's encoding should not.
//!
//! Keep this module focused on keyed-digest construction tests.

use super::{keyed_digest, keyed_digest_hex};

/// The published HMAC-SHA256 test vectors, so the construction is pinned to the
/// standard rather than only to itself. Both cases use a key shorter than the
/// 64-byte block, which is the shape every key here has.
#[test]
fn matches_the_published_hmac_sha256_vectors() {
    // Test case 1: a twenty-byte 0x0b key over "Hi There".
    let key = [0x0b_u8; 20];
    assert_eq!(
        keyed_digest_hex(&key, b"", b"Hi There", 32),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        "HMAC-SHA256 of \"Hi There\" under the 0x0b key"
    );
    // Test case 2: the four-byte "Jefe" key over the same phrase as the
    // matching HMAC-MD5 vector.
    assert_eq!(
        keyed_digest_hex(b"Jefe", b"", b"what do ya want for nothing?", 32),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        "HMAC-SHA256 under the \"Jefe\" key"
    );
}

/// A domain label is part of the hashed message, so two labels under one key
/// never produce the same digest for one message.
#[test]
fn the_domain_label_separates_callers_sharing_a_key() {
    let key = [7_u8; 32];
    let a = keyed_digest_hex(&key, b"domain-a", b"same message", 16);
    let b = keyed_digest_hex(&key, b"domain-b", b"same message", 16);
    assert_ne!(a, b, "two labels must not agree under one key");
    let again = keyed_digest_hex(&key, b"domain-a", b"same message", 16);
    assert_eq!(a, again, "the same label and message are reproducible");
}

/// The width is a parameter: asking for fewer bytes yields a prefix of the
/// wider answer, and asking for the full output yields 32 bytes.
#[test]
fn the_requested_width_is_honoured() {
    let key = [3_u8; 32];
    let wide = keyed_digest(&key, b"d", b"m", 32);
    let narrow = keyed_digest(&key, b"d", b"m", 16);
    assert_eq!(wide.len(), 32, "SHA-256 produces 32 bytes");
    assert_eq!(narrow.len(), 16, "the caller asked for 16");
    assert_eq!(wide[..16], narrow[..narrow.len()], "narrow is a prefix");
    // A caller asking beyond the output length gets the whole digest.
    assert_eq!(
        keyed_digest(&key, b"d", b"m", 64).len(),
        32,
        "the width is clamped to the hash output"
    );
    // A zero width is an empty value rather than a panic.
    let zero_width = keyed_digest(&key, b"d", b"m", 0);
    assert!(zero_width.is_empty(), "{zero_width:?}");
}

/// The digest changes with the key, with the message, and with nothing else:
/// the same inputs give the same lowercase hex on every machine.
#[test]
fn the_digest_is_deterministic_and_sensitive_to_its_inputs() {
    let key = [1_u8; 32];
    let other = [2_u8; 32];
    let base = keyed_digest_hex(&key, b"d", b"m", 16);
    assert_eq!(base, keyed_digest_hex(&key, b"d", b"m", 16));
    assert_ne!(
        base,
        keyed_digest_hex(&other, b"d", b"m", 16),
        "key sensitive"
    );
    assert_ne!(
        base,
        keyed_digest_hex(&key, b"d", b"n", 16),
        "message sensitive"
    );
    assert_eq!(
        base.len(),
        32,
        "16 bytes render as 32 lowercase hex characters"
    );
    assert!(
        base.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "the rendering is lowercase hex, got {base}"
    );
}

/// A key longer than the 64-byte block is hashed down first rather than
/// silently truncated into the pad, so two long keys that share a prefix still
/// produce different digests.
#[test]
fn a_key_longer_than_the_block_is_hashed_first() {
    let mut long = vec![9_u8; 100];
    long.truncate(100);
    let mut differing = long.clone();
    differing[99] = 8;
    let a = keyed_digest_hex(&long, b"d", b"m", 16);
    let b = keyed_digest_hex(&differing, b"d", b"m", 16);
    assert_ne!(
        a, b,
        "a byte past the block length must still reach the digest"
    );
    // A key shorter than the block is used as is, zero-padded.
    let short = [0u8; 32];
    assert_eq!(
        keyed_digest_hex(&short, b"d", b"m", 16),
        keyed_digest_hex(&short, b"d", b"m", 16),
        "a short key is deterministic"
    );
}
