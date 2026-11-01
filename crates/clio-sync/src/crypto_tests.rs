// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the client-side sync encryption adapter: key masking,
//! seal/open roundtrip, wrong-key rejection, and tamper rejection.
//!
//! ## Owns
//! - Unit coverage for `crypto` (`SyncCipherKey` seal/open)
//!
//! ## Does not own
//! - Wire content shapes (`protocol_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the AEAD choice or key derivation changes; they never
//! change it.
//!
//! Keep this module focused on sync cipher tests.
//!
//! ## Nonce uniqueness
//!
//! `key_debug_is_masked_and_nonces_differ` checks the masked `Debug` impl and
//! basic non-determinism only. It passed against the previous clock-derived
//! nonce, and it would stay green against a counter that never increments, so
//! it is not sufficient on its own: the uniqueness claim at the reproduction
//! volume lives in `crypto_nonce_tests`.

/// A fixed associated data for the pure crypto round trips here: these tests
/// seal and open content with no envelope involved.
const TEST_AAD: &[u8] = b"am-sync-test-aad";

#[test]
fn seal_open_roundtrip() {
    let key = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "shared-secret",
    ));
    let plaintext = br#"{"snapshot":{"k":"v"},"gist":"g"}"#;
    let cipher = key.seal(plaintext, TEST_AAD).expect("seal");
    assert_eq!(cipher.alg, "xchacha20poly1305");
    let opened = key.open(&cipher, TEST_AAD).expect("open");
    assert_eq!(opened, plaintext.to_vec());
}

#[test]
fn wrong_key_and_tampering_fail_closed() {
    let key =
        crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret("right"));
    let wrong =
        crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret("wrong"));
    let cipher = key.seal(b"payload", TEST_AAD).expect("seal");
    assert!(wrong.open(&cipher, TEST_AAD).is_err());
    let mut tampered = cipher.clone();
    tampered.ciphertext_b64 = tampered.ciphertext_b64.trim_end_matches('=').to_owned();
    assert!(key.open(&tampered, TEST_AAD).is_err());
    let mut flipped = cipher.clone();
    let mut chars: Vec<char> = flipped.ciphertext_b64.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
    flipped.ciphertext_b64 = chars.into_iter().collect::<String>();
    assert!(key.open(&flipped, TEST_AAD).is_err());
    let mut other_alg = cipher.clone();
    other_alg.alg = "aes".to_owned();
    assert!(key.open(&other_alg, TEST_AAD).is_err());
    let mut bad = cipher.clone();
    bad.ciphertext_b64 = "not base64!!".into();
    assert!(key.open(&bad, TEST_AAD).is_err());
    let mut bad = cipher;
    bad.nonce_b64 = "not base64!!".into();
    assert!(key.open(&bad, TEST_AAD).is_err());
}

#[test]
fn key_debug_is_masked_and_nonces_differ() {
    let key = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret("k"));
    assert_eq!(format!("{key:?}"), "SyncCipherKey(<masked>)");
    let a = key.seal(b"same input", TEST_AAD).expect("seal a");
    let b = key.seal(b"same input", TEST_AAD).expect("seal b");
    assert_ne!(a.nonce_b64, b.nonce_b64);
}

/// The wire identifier: fixed width, stable for one key, different for
/// another, and never equal to another derivation's value for the same key.
#[test]
fn key_id_is_fixed_width_and_distinguishes_keys() {
    let a = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "rotation-key-one",
    ));
    let b = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "rotation-key-two",
    ));
    assert_eq!(a.key_id().len(), 16, "a fixed-width identifier");
    assert_eq!(
        a.key_id(),
        a.key_id(),
        "stable across calls: every holder derives the same value"
    );
    assert_ne!(a.key_id(), b.key_id(), "two keys never share an identifier");
    for ch in a.key_id().chars() {
        assert!(ch.is_ascii_hexdigit(), "hex characters only: {ch}");
    }
    // The property the doc comment claims, now asserted: deriving with the
    // status fingerprint's domain label must never produce the wire value, so
    // pointing the identifier at another derivation's label fails here rather
    // than silently colliding with the status value.
    let fingerprint_labelled = super::truncated_hex_digest(
        b"am-sync-key-fingerprint",
        super::SyncCipherKey::KEY_ID_SEP,
        super::SyncCipherKey::KEY_ID_CHARS,
        a.key.as_ref().as_ref(),
    );
    assert_ne!(
        a.key_id(),
        fingerprint_labelled,
        "the wire identifier never equals a differently labelled derivation of the same key"
    );
}

/// Sealing stamps the sealing key's identifier, so a receiving node can tell
/// which of its keys to try without trying them all.
#[test]
fn seal_stamps_the_sealing_keys_identifier() {
    let key = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "shared-secret",
    ));
    let cipher = key.seal(b"payload", TEST_AAD).expect("seal");
    assert_eq!(cipher.key_id.as_deref(), Some(key.key_id().as_str()));
}

/// A row written before identifiers existed is refused by the client before
/// any open (see `client_rotation_tests`), and the serialized shape carries no
/// `key_id` field when there is none, so the wire shape is unchanged in both
/// directions.
#[test]
fn an_identifier_less_ciphertext_serializes_without_the_field() {
    let key = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "shared-secret",
    ));
    let cipher = key.seal(b"payload", TEST_AAD).expect("seal");
    let stripped = crate::crypto::SyncCipher {
        key_id: None,
        ..cipher.clone()
    };
    let json = serde_json::to_string(&stripped).expect("serialize");
    assert!(
        !json.contains("key_id"),
        "an absent identifier is absent from the wire: {json}"
    );
    let old_reader_shape = serde_json::from_str::<crate::crypto::SyncCipher>(&json).expect("parse");
    assert_eq!(old_reader_shape.key_id, None);
    let stamped = serde_json::to_string(&cipher).expect("serialize");
    assert!(
        stamped.contains("key_id"),
        "a stamped row carries it: {stamped}"
    );
}

/// Two instances built from one secret derive the same identifier and open
/// each other's seals, which is what lets two peers hold only the secret
/// string and still read the same rows: the derivation may depend on nothing
/// but the secret itself.
#[test]
fn two_instances_of_one_secret_share_the_identifier_and_each_others_seals() {
    let secret = crate::credential_fixtures::secret("peer-shared-secret");
    let a = crate::crypto::SyncCipherKey::from_secret(&secret);
    let b = crate::crypto::SyncCipherKey::from_secret(&secret);
    assert_eq!(a.key_id(), b.key_id(), "one secret, one identifier");
    let cipher = a.seal(b"peer payload", TEST_AAD).expect("seal");
    assert_eq!(
        b.open(&cipher, TEST_AAD).expect("open"),
        b"peer payload".to_vec(),
        "a peer re-deriving the same secret opens the seal"
    );
}

/// The key bytes are stretched, not the retired fast digest: if derivation
/// ever regresses to the single unsalted SHA-256 pass, the inequality below
/// fails and this test names the path it fell back to.
#[test]
fn the_derived_key_differs_from_the_legacy_sha256_digest() {
    let secret = crate::credential_fixtures::secret("legacy-kdf-path");
    let key = crate::crypto::SyncCipherKey::from_secret(&secret);
    let legacy_hex = clio_store::sha256_hex(secret.as_bytes(), "am-sync-key");
    let legacy_digest = super::hex_to_bytes(&legacy_hex);
    assert_ne!(
        key.key_bytes(),
        legacy_digest.as_slice(),
        "the key is stretched, not the legacy single SHA-256 digest"
    );
}

/// The AEAD seals under the sealing role's derived subkey, never under the sync
/// key itself, so the digest role and the handle role cannot be reached from
/// the key that protects content.
#[test]
fn the_aead_never_seals_under_the_sync_key_itself() {
    let role_secret = crate::credential_fixtures::secret("seal-role-key");
    let key = crate::crypto::SyncCipherKey::from_secret(&role_secret);
    let seal_key = key.seal_key();
    assert_ne!(
        seal_key,
        key.key_bytes().to_vec(),
        "the sealing role derives its own key from the sync key"
    );
    assert_eq!(seal_key.len(), 32, "a full SHA-256 output");
    // A peer holding the same sync key derives the same sealing key, so
    // ciphertext written by one opens on the other.
    let peer = crate::crypto::SyncCipherKey::from_secret(&role_secret);
    assert_eq!(
        seal_key,
        peer.seal_key(),
        "reproducible from the sync key alone"
    );
}

/// Content sealed under the raw sync key by an earlier build no longer opens:
/// the raw-key fallback is retired, so such a row is dead-lettered at apply
/// time instead of read. Everything this build seals uses the derived role
/// key, which the raw key alone never opens.
#[test]
fn content_sealed_under_the_raw_sync_key_is_refused() {
    use base64::Engine as _;
    use chacha20poly1305::{
        XChaCha20Poly1305, XNonce,
        aead::{Aead, KeyInit, Payload},
    };
    let key = crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "legacy-raw-key",
    ));
    // Seal exactly the way the earlier build did: the sync key itself.
    let nonce: [u8; 24] = rand::random();
    let aead = XChaCha20Poly1305::new(key.key_bytes().to_vec().as_slice().into());
    let sealed = aead
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: b"legacy payload",
                aad: TEST_AAD,
            },
        )
        .expect("seal under the raw key");
    let legacy = crate::crypto::SyncCipher {
        alg: "xchacha20poly1305".into(),
        key_id: Some(key.key_id()),
        nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce),
        ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(sealed),
    };
    assert!(
        key.open(&legacy, TEST_AAD).is_err(),
        "a row sealed under the raw key is refused, not opened by a fallback"
    );
    // And a row this build seals does not open under the raw key alone, so
    // the retired fallback cannot be mistaken for the sealing path.
    let mine = key.seal(b"new payload", TEST_AAD).expect("seal");
    assert!(
        XChaCha20Poly1305::new(key.key_bytes().to_vec().as_slice().into())
            .decrypt(
                XNonce::from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(&mine.nonce_b64)
                        .expect("nonce"),
                ),
                Payload {
                    msg: &base64::engine::general_purpose::STANDARD
                        .decode(&mine.ciphertext_b64)
                        .expect("body"),
                    aad: TEST_AAD,
                },
            )
            .is_err(),
        "a newly sealed row is not readable with the raw sync key"
    );
}

/// The AEAD authenticates the associated data: the same ciphertext opened
/// under a different envelope is refused, which is what stops a relay from
/// retargeting valid ciphertext at another mutation.
#[test]
fn a_different_associated_data_fails_authentication() {
    let key =
        crate::crypto::SyncCipherKey::from_secret(&crate::credential_fixtures::secret("aad-key"));
    let cipher = key.seal(b"envelope-bound payload", TEST_AAD).expect("seal");
    assert!(
        key.open(&cipher, b"am-sync-test-aab").is_err(),
        "another envelope must not open this ciphertext"
    );
    assert!(
        key.open(&cipher, b"").is_err(),
        "an empty envelope is refused"
    );
    assert!(
        key.open(&cipher, TEST_AAD).is_ok(),
        "the envelope it was sealed under still opens it"
    );
}
