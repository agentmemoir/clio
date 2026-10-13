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
use std::collections::HashSet;

/// Sealing threads for the uniqueness test.
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
                    let sealed = key.seal(payloads[i % payloads.len()]).expect("seal");
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
        let sealed = key.seal(b"shape probe").expect("seal");
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

/// The fixture generated by the build before the nonce source change is
/// refused: it predates key identifiers and the sealing role's subkey, and the
/// old-key opening that used to read it is retired by design. Its nonce still
/// decodes to 24 hex-text bytes, the old derivation's signature, which also
/// proves the fixture is the real one.
#[test]
fn ciphertext_sealed_before_the_nonce_change_is_refused() {
    const PRE_CHANGE_SECRET: &str = "pre-csprng-sync-fixture-key";
    const PRE_CHANGE_NONCE_B64: &str = "ZjViMTE0NWU4NDYxY2U4NDEwNDNjOGYy";
    const PRE_CHANGE_CIPHERTEXT_B64: &str = "MnYLk4FPMqlQLdAgd8ZKJtUS810B3eDelZ4TrmaDLv1rXr2/41gEYPToS6LVY/xp3S+mmzjG0XdW1oit4oqdod5UgaQ0nCBudtlyqTg/aPgTGG+iPv24";

    let nonce = base64::engine::general_purpose::STANDARD
        .decode(PRE_CHANGE_NONCE_B64)
        .expect("fixture nonce is base64");
    assert_eq!(nonce.len(), 24, "fixture nonce is 24 bytes");
    assert!(
        nonce.iter().all(u8::is_ascii_hexdigit),
        "fixture nonce carries the previous derivation's hex-text shape",
    );

    let key = SyncCipherKey::from_secret(PRE_CHANGE_SECRET);
    let sealed = SyncCipher {
        alg: "xchacha20poly1305".to_owned(),
        key_id: None,
        nonce_b64: PRE_CHANGE_NONCE_B64.to_owned(),
        ciphertext_b64: PRE_CHANGE_CIPHERTEXT_B64.to_owned(),
    };
    let opened = key.open(&sealed);
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
