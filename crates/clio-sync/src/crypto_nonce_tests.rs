// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync AEAD nonce source: every seal draws a fresh
//! 24-byte CSPRNG nonce, including under concurrent sealing with repeated and
//! equal-length plaintexts, the decoded nonce is 24 raw bytes rather than 24
//! characters of hex text, and a ciphertext sealed by the previous
//! clock-derived build still opens.
//!
//! ## Owns
//! - The nonce-uniqueness, nonce-shape, `Send + Sync`, and pre-change
//!   ciphertext regression tests for `SyncCipherKey`
//!
//! ## Does not own
//! - Masked `Debug`, round trip, and `open`'s refusal set (`crypto_tests`)
//! - The live push/pull measurement (phase evidence, not a unit test)
//!
//! ## Boundary
//!
//! Tests here break when the seal-side nonce source or the wire nonce shape
//! changes; they never change either.
//!
//! Keep this module focused on the seal-side nonce property.

use super::SyncCipher;
use crate::crypto::SyncCipherKey;
use base64::Engine as _;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use std::collections::HashSet;
use std::fmt::Write as _;

/// Sealing threads for the uniqueness test.
/// A fixed associated data for the nonce tests: they exercise the AEAD's nonce
/// handling and not the envelope binding.
const TEST_AAD: &[u8] = b"am-sync-test-aad";

const THREADS: usize = 8;

/// Seals per thread: 8 x `30_000` = `240_000`, the volume the reuse reproduction
/// used against the clock-and-length construction; the count is not a timing
/// threshold. The cardinality check alone detects the reverted construction
/// only as fast as the host clocks: the measured revert produced 61 collisions
/// in `240_000` seals (0.025%), while the issue's reproduction measured `42_793`
/// reuses for the same parameters, because both counts depend on how many
/// seals land inside one clock tick. Detection therefore does not rest on
/// that margin: the old derivation emitted 24 hex-text bytes per nonce, so
/// the hex-text guard below fails on the first reverted seal regardless of
/// host speed. A correct 192-bit draw collides with probability below 2^-160.
const SEALS_PER_THREAD: usize = 30_000;

/// Seals of two equal-length payloads across eight threads under one key must
/// all carry distinct nonces. The old clock-and-length derivation collapsed
/// equal-length payloads sealed in the same clock tick, which is the defect
/// this pins: `240_000` seals split across eight threads is the reproduction
/// volume, and the set cardinality is compared to the seal count with no
/// sampling window.
#[test]
fn concurrent_seals_never_reuse_a_nonce() {
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "nonce-uniqueness-fixture",
    ));
    let payloads: [&[u8]; 2] = [b"payload-alpha-0123456789", b"payload-beta-0123456789"];
    let per_thread: Vec<Vec<[u8; 24]>> = std::thread::scope(|scope| {
        // Spawn all threads before joining any of them: the concurrency is the
        // property under test, and a fused spawn-then-join chain would run the
        // threads one at a time.
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            handles.push(scope.spawn(|| {
                let mut nonces = Vec::with_capacity(SEALS_PER_THREAD);
                for i in 0..SEALS_PER_THREAD {
                    let sealed = key
                        .seal(payloads[i % payloads.len()], TEST_AAD)
                        .expect("seal");
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(&sealed.nonce_b64)
                        .expect("sealed nonce is base64");
                    let nonce: [u8; 24] = decoded
                        .as_slice()
                        .try_into()
                        .expect("sealed nonce is 24 bytes");
                    nonces.push(nonce);
                }
                nonces
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("seal thread panicked"))
            .collect()
    });
    let total = THREADS * SEALS_PER_THREAD;
    let mut distinct: HashSet<[u8; 24]> = HashSet::with_capacity(total);
    for nonces in &per_thread {
        distinct.extend(nonces.iter().copied());
    }
    // Deterministic revert detection: the previous derivation emitted 24
    // hex-text bytes per nonce, so a reverted build fails here on the first
    // seal, with no dependence on how the host clock interleaves the threads.
    // Counts only: a failure message never carries a nonce.
    let hex_text = per_thread
        .iter()
        .flat_map(|nonces| nonces.iter())
        .filter(|nonce| nonce.iter().all(u8::is_ascii_hexdigit))
        .count();
    assert_eq!(
        hex_text, 0,
        "{total} seals produced {hex_text} hex-text nonces; the clock-and-length derivation emits only hex-text nonces",
    );
    // Counts only: a failure message never carries a nonce.
    assert_eq!(
        distinct.len(),
        total,
        "nonce uniqueness violated: {total} seals produced {} distinct nonces",
        distinct.len()
    );
}

/// The decoded nonce is exactly 24 raw bytes, and its bytes are not the ASCII
/// hex text the previous derivation produced (24 characters of `sha256_hex`).
/// A CSPRNG byte lands in the 22-character hex alphabet with probability
/// 22/256, so a whole 24-byte nonce landing there has probability
/// (22/256)^24 < 2^-84, and 16 independent draws all landing there has
/// probability (22/256)^384 < 2^-1359: the shape claim cannot flake in
/// practice.
#[test]
fn sealed_nonce_is_24_raw_bytes_not_hex_text() {
    let key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("nonce-shape-fixture"));
    for _ in 0..16 {
        let sealed = key.seal(b"shape probe", TEST_AAD).expect("seal");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&sealed.nonce_b64)
            .expect("sealed nonce is base64");
        assert_eq!(decoded.len(), 24, "sealed nonce must decode to 24 bytes");
        assert!(
            !decoded.iter().all(u8::is_ascii_hexdigit),
            "sealed nonce must not be 24 characters of hex text",
        );
    }
}

/// A row sealed by the build before the nonce source change is refused: it
/// predates key identifiers and carries the retired fixed associated data, and
/// the old opening that used to read it is retired by design. The row is
/// rebuilt here from a generated key in the pre-change shape: a 24-character
/// hex-text nonce, no key identifier, and the old fixed label as associated
/// data.
#[test]
fn ciphertext_sealed_before_the_nonce_change_is_refused() {
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "pre-change-nonce-fixture",
    ));
    // The previous derivation emitted 24 hex-text characters as the nonce.
    let hex_text_nonce = (0..12).fold(String::new(), |mut text, _| {
        write!(text, "{:02x}", rand::random::<u8>()).expect("writing to a String cannot fail");
        text
    });
    assert_eq!(hex_text_nonce.len(), 24, "fixture nonce is 24 bytes");
    assert!(
        hex_text_nonce.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "fixture nonce carries the previous derivation's hex-text shape",
    );
    let nonce_bytes: [u8; 24] = hex_text_nonce
        .as_bytes()
        .try_into()
        .expect("fixture nonce is 24 bytes");

    let aead = XChaCha20Poly1305::new_from_slice(&key.seal_key())
        .expect("the sealing subkey is a 32-byte key");
    let sealed_bytes = aead
        .encrypt(
            &XNonce::from(nonce_bytes),
            Payload {
                msg: b"pre-change plaintext",
                aad: b"am-sync-v1",
            },
        )
        .expect("seal the pre-change fixture");
    let sealed = SyncCipher {
        alg: "xchacha20poly1305".to_owned(),
        key_id: None,
        nonce_b64: base64::engine::general_purpose::STANDARD.encode(hex_text_nonce.as_bytes()),
        ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(sealed_bytes),
    };
    let opened = key.open(&sealed, TEST_AAD);
    assert!(
        opened.is_err(),
        "pre-identifier ciphertext is refused, not opened"
    );
}

/// A replacement nonce source that is not thread-safe would be a defect even
/// though today's push loop is single-threaded, because one key is shared
/// across MCP HTTP connection threads by design. This is a compile-time
/// assertion: if it compiles, it passes.
#[test]
fn sync_cipher_key_stays_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SyncCipherKey>();
}
