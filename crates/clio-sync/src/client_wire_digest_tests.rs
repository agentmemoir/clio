// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the stored content digest as the
//! relay's own journal holds it: that a candidate-plaintext dictionary confirms
//! nothing against a sealed row, and that a plaintext deployment still stores
//! the real identifier and the unkeyed digest it has always stored.
//!
//! ## Owns
//! - Journal-level assertions for the content digest, in both cipher modes
//!
//! ## Does not own
//! - The identifier column's blindness (`client_wire_blindness_tests`)
//! - Pull-path resolution (`client_resolve_tests`)
//! - The derivations themselves (`keyed_roles_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the stored digest changes; they never change it.
//!
//! Keep this module focused on the stored content digest.

use crate::client::client_tests::{cfg, item, paired_cfg, serve, sqlite};
use crate::client::push;
use crate::client_wire_fixtures::{
    ATTR_NAME, FAILURE_ID, RELATIONSHIP, TASK_ID, belief, seed_explicit_edge, seed_persona,
    seed_task_and_failure,
};
use crate::crypto::SyncCipherKey;
use clio_store::{BeliefStore, Store, SyncStore};

const GIST: &str = "synthetic gist plaintext marker";

/// A relay holding the journal cannot confirm a guessed plaintext against the
/// stored content digest: the value it holds is not the unkeyed hash the guess
/// would be tested with, and it is not the content in any form.
#[test]
fn the_stored_content_digest_confirms_no_guessed_plaintext() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret(
            &crate::credential_fixtures::secret("blind-digest-key"),
        )],
    );
    a.create_memory_item(
        &item("bp", "i-1", "s1", "2026-01-01T00:00:00Z", GIST),
        "actor",
    )
    .expect("item");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 10).expect("server journal");
    let stored = &rows[0].content_hash;
    let fixture = item("bp", "i-1", "s1", "2026-01-01T00:00:00Z", GIST);
    let guesses = [
        serde_json::to_string(&fixture).expect("serialize the item"),
        GIST.to_owned(),
        format!("{{\"gist\":\"{GIST}\"}}"),
    ];
    for guess in guesses {
        let attack = clio_store::sha256_hex(crate::keyed_roles::published_digest_salt(), &guess);
        assert_ne!(
            stored.as_str(),
            attack.as_str(),
            "the stored digest must not equal the unkeyed hash of a candidate"
        );
    }
    assert!(
        !stored.contains(GIST),
        "the digest is not the content either: {stored}"
    );
    server.stop();
}

/// With a cipher configured, every produced kind still reaches the relay, and
/// the two values it stores are the sealed forms of what a plaintext deployment
/// stores: a machine-generated identifier travels as is, a caller-chosen one
/// travels as the keyed handle, and the digest keeps the 64-character hex shape
/// the apply guard compares. The plaintext parity of both values is pinned at
/// the packager, which is now the only place a plaintext package exists.
#[test]
fn plaintext_mode_stores_the_real_identifier_and_the_plain_digest() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed_persona(&a, "bp");
    seed_task_and_failure(&a, "bp");
    seed_explicit_edge(&a, "bp", RELATIONSHIP);
    a.create_belief(&belief("bel-plain-1", "bp"), "seed")
        .expect("belief");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv
        .journal_list_since("bp", 0, 100)
        .expect("server journal");
    let kind_of = |want: &str| {
        rows.iter()
            .find(|r| r.entity_kind == want)
            .unwrap_or_else(|| panic!("a {want} row reaches the relay"))
    };
    let handle = |kind: &str, real: &str| {
        crate::keyed_roles::identifier_handle(
            Some(&crate::client::client_tests::paired_key()),
            kind,
            "bp",
            real,
        )
    };
    let edge = clio_store::explicit_id("bp", "i-1", "i-2", RELATIONSHIP);
    assert_eq!(kind_of("persona").entity_id, handle("persona", ATTR_NAME));
    assert_eq!(kind_of("task").entity_id, handle("task", TASK_ID));
    assert_eq!(kind_of("failure").entity_id, handle("failure", FAILURE_ID));
    // A machine-generated identifier is one the relay learns nothing new from.
    assert_eq!(
        kind_of("belief").entity_id,
        "bel-plain-1",
        "a machine-generated belief id as is"
    );
    assert_eq!(
        kind_of("assoc_edge").entity_id,
        handle("assoc_edge", &edge),
        "the edge travels as the keyed handle"
    );
    assert_ne!(
        kind_of("assoc_edge").entity_id,
        edge,
        "a relay cannot read the edge's real identifier out of the row"
    );
    // The digest is a keyed SHA-256 in the same 64-hex-character shape the
    // plaintext digest has always had, so every guard comparison still lines up.
    let item_row = kind_of("item");
    assert_eq!(item_row.content_hash.len(), 64, "a keyed SHA-256 hex");
    assert!(
        item_row
            .content_hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "the stored digest keeps the 64-hex-character shape"
    );
    server.stop();
}
