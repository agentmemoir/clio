// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the four sync key roles and the three
//! relay-facing values they produce: that the four subkeys are distinct and
//! each reproducible from one sync key, that the content digest and the
//! idempotency key are keyed only when a cipher is configured, that the
//! identifier handle is stable per (kind, bank, identifier) and different along
//! each of those three axes, and that the handled record kinds are the ones
//! carrying caller-chosen text.
//!
//! ## Owns
//! - The tests for `role_subkey`, `content_digest`, `identifier_handle`,
//!   `mutation_id`, and `kind_uses_handle`
//!
//! ## Does not own
//! - Seal and open correctness and nonce behaviour (`crypto_nonce_tests`,
//!   `crypto_tests`)
//! - Reading the identifier back out of a payload (`handle_resolution_tests`)
//! - The relay's stored journal bytes (`client`'s wire tests)
//!
//! ## Boundary
//!
//! Changes to the derived roles or to either derived value should normally
//! require changes here. Changes to what a payload contains should not.
//!
//! Keep this module focused on key-role and derived-value tests.

use super::{
    HANDLE_BYTES, ROLE_DIGEST, ROLE_HANDLE, ROLE_MUTATION_ID, ROLE_SEAL, content_digest,
    identifier_handle, kind_uses_handle, mutation_id, role_subkey,
};
use crate::crypto::SyncCipherKey;
use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_ITEM,
    ENTITY_KIND_PERSONA, ENTITY_KIND_TASK, ENTITY_KIND_TOMBSTONE, ENTITY_KIND_TRIPLE,
};

/// Every pair of the four roles derives a different subkey, and each is
/// reproducible from that key alone — which is what lets a peer that shares the
/// key derive the same value and a peer that lacks it derive nothing.
#[test]
fn the_four_roles_derive_distinct_reproducible_subkeys() {
    let roles_secret = crate::credential_fixtures::secret("roles-key");
    let key = SyncCipherKey::from_secret(&roles_secret);
    let same_key = SyncCipherKey::from_secret(&roles_secret);
    let other_key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("roles-other-key"));
    let subkeys = [
        (ROLE_SEAL, role_subkey(&key, ROLE_SEAL)),
        (ROLE_DIGEST, role_subkey(&key, ROLE_DIGEST)),
        (ROLE_HANDLE, role_subkey(&key, ROLE_HANDLE)),
        (ROLE_MUTATION_ID, role_subkey(&key, ROLE_MUTATION_ID)),
    ];
    for (label, subkey) in &subkeys {
        assert_eq!(subkey.len(), 32, "a role subkey is a full SHA-256 output");
        assert_eq!(
            *subkey,
            role_subkey(&same_key, label),
            "reproducible from the same key on any node"
        );
        assert_ne!(
            *subkey,
            role_subkey(&other_key, label),
            "a different sync key derives a different subkey"
        );
    }
    for (i, (label_a, key_a)) in subkeys.iter().enumerate() {
        for (label_b, key_b) in subkeys.iter().skip(i + 1) {
            assert_ne!(
                key_a,
                key_b,
                "the roles {} and {} must not share key material",
                String::from_utf8_lossy(label_a),
                String::from_utf8_lossy(label_b)
            );
        }
    }
}

/// With no cipher the content digest is the plain unkeyed hash, so every
/// existing plaintext deployment keeps comparing equal against the rows it
/// already stored.
#[test]
fn the_content_digest_without_a_cipher_is_the_plain_hash() {
    let content = r#"{"gist":"unchanged","n":1}"#;
    let plain = content_digest(None, content);
    assert_eq!(
        plain,
        clio_store::sha256_hex(b"am-sync-hash", content),
        "plaintext mode keeps the pre-keyed value byte for byte"
    );
    assert_eq!(plain.len(), 64, "an unkeyed SHA-256 is 64 hex characters");
}

/// With a cipher the digest is keyed, deterministic across peers sharing the
/// key, and different under a different key — so a relay cannot hash a
/// candidate plaintext and confirm it.
#[test]
fn the_content_digest_with_a_cipher_is_keyed_and_peer_stable() {
    let digest_secret = crate::credential_fixtures::secret("digest-key");
    let key = SyncCipherKey::from_secret(&digest_secret);
    let peer = SyncCipherKey::from_secret(&digest_secret);
    let stranger =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("digest-other-key"));
    let content = r#"{"gist":"candidate plaintext"}"#;
    let mine = content_digest(Some(&key), content);
    assert_eq!(mine, content_digest(Some(&peer), content), "peers agree");
    assert_ne!(
        mine,
        content_digest(Some(&stranger), content),
        "a node without the key derives a different value"
    );
    assert_ne!(mine, content_digest(None, content), "keyed is not plain");
    assert_ne!(
        mine,
        clio_store::sha256_hex(crate::keyed_roles::PLAINTEXT_DIGEST_SALT, content),
        "a candidate-plaintext dictionary must not confirm"
    );
}

/// With no cipher the idempotency key is the unkeyed hash of the same preimage,
/// byte for byte what a plaintext deployment has always stored, so every
/// existing redelivery check against a stored row still matches.
#[test]
fn the_mutation_id_without_a_cipher_is_the_plain_hash() {
    let plain = mutation_id(None, "task-42", "2026-01-01", "upsert");
    assert_eq!(
        plain,
        clio_store::sha256_hex(b"am-sync-mut", "task-42|2026-01-01|upsert"),
        "plaintext mode keeps the pre-keyed value byte for byte"
    );
    assert_eq!(plain.len(), 64, "an unkeyed SHA-256 is 64 hex characters");
}

/// With a cipher the idempotency key is keyed, peer-stable, and different for a
/// different record — so a relay cannot hash a guessed identifier and confirm it
/// against the stored key.
#[test]
fn the_mutation_id_with_a_cipher_is_keyed_and_peer_stable() {
    let mutid_secret = crate::credential_fixtures::secret("mutid-key");
    let key = SyncCipherKey::from_secret(&mutid_secret);
    let peer = SyncCipherKey::from_secret(&mutid_secret);
    let stranger =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("mutid-other-key"));
    let mine = mutation_id(Some(&key), "task-42", "2026-01-01", "upsert");
    assert_eq!(
        mine,
        mutation_id(Some(&peer), "task-42", "2026-01-01", "upsert"),
        "peers sharing a key agree, so a redelivery still matches"
    );
    assert_ne!(
        mine,
        mutation_id(Some(&stranger), "task-42", "2026-01-01", "upsert"),
        "a node without the key derives a different key"
    );
    assert_ne!(
        mine,
        clio_store::sha256_hex(
            crate::keyed_roles::PLAINTEXT_MUTATION_ID_SALT,
            "task-42|2026-01-01|upsert",
        ),
        "a guessed identifier must not confirm against the stored key"
    );
    assert_ne!(
        mine,
        mutation_id(Some(&key), "task-43", "2026-01-01", "upsert"),
        "two records that must stay separate get two keys"
    );
    assert_ne!(
        mine,
        mutation_id(Some(&key), "task-42", "2026-01-02", "upsert"),
        "the stamp is part of the preimage, so two versions differ"
    );
}

/// A handle is fixed-width lowercase hex with no letter prefix, and it is the
/// same value for the same (kind, bank, identifier) every time.
#[test]
fn a_handle_is_fixed_width_lowercase_hex_and_stable() {
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("handle-key"));
    let handle = identifier_handle(Some(&key), ENTITY_KIND_TASK, "b1", "task-42");
    assert_eq!(
        handle.len(),
        HANDLE_BYTES * 2,
        "a fixed width on the wire, rendered as hex"
    );
    assert!(
        handle
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "lowercase hex with no letter prefix, got {handle}"
    );
    assert_eq!(
        handle,
        identifier_handle(Some(&key), ENTITY_KIND_TASK, "b1", "task-42"),
        "stable for the same kind, bank and identifier"
    );
}

/// The three axes that must never collide: a different identifier, a different
/// bank, and a different kind. Two records that share any two of them must
/// still get different handles, because a shared handle would merge them.
#[test]
fn handles_differ_along_each_of_the_three_axes() {
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("collision-key"));
    let base = identifier_handle(Some(&key), ENTITY_KIND_TASK, "b1", "task-42");
    assert_ne!(
        base,
        identifier_handle(Some(&key), ENTITY_KIND_TASK, "b1", "task-43"),
        "two task ids"
    );
    assert_ne!(
        base,
        identifier_handle(Some(&key), ENTITY_KIND_TASK, "b2", "task-42"),
        "the same task id in two banks"
    );
    assert_ne!(
        base,
        identifier_handle(Some(&key), ENTITY_KIND_FAILURE, "b1", "task-42"),
        "the same identifier under two kinds"
    );
    // Two relationship labels on one endpoint pair reach the handle only
    // through their edge ids, so the handles differ.
    let edge_a = "expl\u{1f}b1\u{1f}i-1\u{1f}i-2\u{1f}mentor";
    let edge_b = "expl\u{1f}b1\u{1f}i-1\u{1f}i-2\u{1f}reports_to";
    assert_ne!(
        identifier_handle(Some(&key), ENTITY_KIND_ASSOC, "b1", edge_a),
        identifier_handle(Some(&key), ENTITY_KIND_ASSOC, "b1", edge_b),
        "two relationship labels on one endpoint pair"
    );
}

/// With no cipher the wire identifier is the real identifier: plaintext mode is
/// unchanged for every field.
#[test]
fn without_a_cipher_the_wire_identifier_is_the_real_one() {
    assert_eq!(
        identifier_handle(None, ENTITY_KIND_TASK, "b1", "task-42"),
        "task-42"
    );
}

/// The handled kinds are exactly the four whose business identifier carries a
/// caller-chosen string. The machine-generated identifiers and the kind with no
/// producer are not handled, and extending it to them needs a documented reason.
#[test]
fn only_caller_chosen_identifier_kinds_use_a_handle() {
    for kind in [
        ENTITY_KIND_PERSONA,
        ENTITY_KIND_TASK,
        ENTITY_KIND_FAILURE,
        ENTITY_KIND_ASSOC,
    ] {
        assert!(kind_uses_handle(kind), "{kind} carries caller-chosen text");
    }
    for kind in [
        ENTITY_KIND_ITEM,
        ENTITY_KIND_TRIPLE,
        ENTITY_KIND_BELIEF,
        ENTITY_KIND_TOMBSTONE,
    ] {
        assert!(!kind_uses_handle(kind), "{kind} is machine-generated");
    }
}
