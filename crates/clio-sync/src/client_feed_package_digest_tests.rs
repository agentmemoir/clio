// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the content digest the packaging
//! helpers put on the wire: that it is the plain unkeyed hash in plaintext mode,
//! that it is keyed and peer-stable with a cipher, that it follows the content
//! and never the identifier, and that no candidate plaintext can be confirmed
//! against the stored value.
//!
//! ## Owns
//! - Packaging-level assertions for the content digest
//!
//! ## Does not own
//! - The wire identifier (`client_feed_package_handle_tests`)
//! - What the relay can read off its stored bytes
//!   (`client_wire_blindness_tests`)
//! - The derivations themselves (`keyed_roles_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the content digest changes; they never change it.
//!
//! Keep this module focused on the packaged content digest.

use crate::client_feed::package::task_mutation;
use crate::client_feed_package_fixtures::{
    STAMP, TASK_ID, carrier, plain_cfg, sealed_cfg, task_record,
};
use crate::protocol::SyncContent;
use serde_json::json;

fn digest_for(cfg: &crate::client::SyncClientConfig) -> String {
    task_mutation(
        cfg,
        crate::client_feed_package_fixtures::BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task")
    .content_hash
}

/// With no cipher the digest is the plain unkeyed hash of the same serialized
/// plaintext, byte for byte what a plaintext deployment has always stored, so
/// every existing apply-guard comparison against a stored row still matches.
#[test]
fn plaintext_mode_keeps_the_plain_digest() {
    let cfg = plain_cfg();
    let digest = digest_for(&cfg);
    let payload = task_mutation(
        &cfg,
        crate::client_feed_package_fixtures::BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    let SyncContent::Plain(value) = &payload.content else {
        panic!("plaintext mode seals nothing");
    };
    assert_eq!(
        digest,
        clio_store::sha256_hex(
            b"am-sync-hash",
            &serde_json::to_string(value).expect("text")
        ),
        "the digest is the unkeyed hash of the same serialized plaintext"
    );
    assert_eq!(digest.len(), 64, "a full SHA-256 hex digest");
}

/// With a cipher the digest is keyed, so a relay holding it cannot confirm a
/// candidate plaintext: the value it holds is not the hash the guess would be
/// tested with.
#[test]
fn a_cipher_makes_the_digest_unconfirmable_by_a_guess() {
    let payload = task_mutation(
        &sealed_cfg("pkg-digest-blind"),
        crate::client_feed_package_fixtures::BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    let candidate =
        serde_json::to_string(&json!({"guess": "a candidate plaintext"})).expect("text");
    assert_ne!(
        payload.content_hash,
        clio_store::sha256_hex(b"am-sync-hash", &candidate),
        "the stored digest is not the unkeyed hash of a candidate"
    );
    assert!(
        !payload.content_hash.contains(TASK_ID),
        "and it is not the content either"
    );
}

/// The digest is a pure function of the serialized content under the held key:
/// the same bytes give the same digest, different bytes give a different one,
/// and the identifier string never enters it.
#[test]
fn the_digest_follows_the_content_not_the_identifier() {
    let cfg = sealed_cfg("pkg-digest-content");
    let base = digest_for(&cfg);
    assert_eq!(base, digest_for(&cfg), "the same content, the same digest");
    let other_record = task_mutation(
        &cfg,
        crate::client_feed_package_fixtures::BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-2"),
        &carrier("car-task"),
    )
    .expect("task");
    assert_ne!(
        base, other_record.content_hash,
        "different content, a different digest"
    );
    let text = serde_json::to_string(&json!({"note": "digest input"})).expect("text");
    assert!(
        !crate::keyed_roles::content_digest(cfg.cipher.first(), &text).contains(TASK_ID),
        "the identifier never enters the digest"
    );
}

/// Two peers holding the same key derive the same digest from the same record,
/// which is what keeps the apply guard's wire comparison matching on both
/// sides; a node without the key derives something else.
#[test]
fn two_peers_sharing_a_key_derive_the_same_digest() {
    let mine = digest_for(&sealed_cfg("pkg-digest-peer"));
    let theirs = digest_for(&sealed_cfg("pkg-digest-peer"));
    let stranger = digest_for(&sealed_cfg("pkg-digest-stranger"));
    assert_eq!(mine, theirs, "peers sharing a key agree");
    assert_ne!(
        mine, stranger,
        "a node without the key cannot reproduce the digest"
    );
}
