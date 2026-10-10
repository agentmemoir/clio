// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what a relay holding the journal can
//! read from a sealed push of each handled record kind: that no persona
//! attribute name, task id, failure id, or relationship label appears anywhere
//! on the stored row, including in its idempotency key or admission receipt,
//! that no guessed identifier confirms against a stored
//! digest-shaped value, that plaintext mode stores the unkeyed idempotency key
//! it always stored, and that a machine-generated identifier is left as it is.
//! Every assertion is made against the server's own stored bytes.
//!
//! ## Owns
//! - Journal-level assertions for the sealed wire shape, read from the server's
//!   own store rather than from any in-process helper
//! - The guess-confirmation assertions, which hash candidate preimages against
//!   the stored columns rather than searching the row for a substring
//!
//! ## Does not own
//! - The stored content digest and plaintext parity (`client_wire_digest_tests`)
//! - Pull-path resolution (`client_resolve_tests`)
//! - The derivations themselves (`keyed_roles_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the wire shape changes; they never change it.
//!
//! Keep this module focused on relay blindness against stored bytes.

use crate::client::client_tests::{cfg, serve, sqlite};
use crate::client::push;
use crate::client_wire_fixtures::{
    ATTR_NAME, FAILURE_ID, PREF_NAME, RELATIONSHIP, TASK_ID, belief, seed_explicit_edge,
    seed_persona, seed_task_and_failure, stored_bytes,
};
use crate::crypto::SyncCipherKey;
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{BeliefStore, SyncStore};
use serde_json::Value;

/// The relay's stored row for a sealed persona push names neither the stable
/// attribute key nor the preference key, in the identifier column or anywhere
/// else on the row.
#[test]
fn a_sealed_persona_push_reaches_the_relay_with_no_attribute_name() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-persona-key")],
    );
    seed_persona(&a, "bp");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    let persona: Vec<_> = rows.iter().filter(|r| r.entity_kind == "persona").collect();
    assert_eq!(persona.len(), 2, "both persona shapes reach the relay");
    for row in persona {
        let bytes = stored_bytes(row);
        assert!(
            row.client_encrypted,
            "the persona row is sealed, so the attribute key cannot be in the clear body"
        );
        assert!(
            !bytes.contains(ATTR_NAME),
            "the attribute name must not reach the relay: {bytes}"
        );
        assert!(
            !bytes.contains(PREF_NAME),
            "the preference name must not reach the relay: {bytes}"
        );
    }
    server.stop();
}

/// The relay's stored row for a sealed task push contains neither the caller's
/// task id nor the structured row's own id.
#[test]
fn a_sealed_task_push_reaches_the_relay_with_no_task_id() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-task-key")],
    );
    seed_task_and_failure(&a, "bp");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    let task = rows
        .iter()
        .find(|r| r.entity_kind == "task")
        .expect("the task row reaches the relay");
    let bytes = stored_bytes(task);
    assert!(task.client_encrypted, "the task row is sealed");
    assert!(
        !bytes.contains(TASK_ID),
        "the caller task id must not reach the relay: {bytes}"
    );
    server.stop();
}

/// The relay's stored row for a sealed failure push contains neither the
/// failure id nor the task id that failure id embeds.
#[test]
fn a_sealed_failure_push_reaches_the_relay_with_no_failure_or_task_id() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-failure-key")],
    );
    seed_task_and_failure(&a, "bp");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    let failure = rows
        .iter()
        .find(|r| r.entity_kind == "failure")
        .expect("the failure row reaches the relay");
    let bytes = stored_bytes(failure);
    assert!(failure.client_encrypted, "the failure row is sealed");
    assert!(
        !bytes.contains(FAILURE_ID),
        "the failure id must not reach the relay: {bytes}"
    );
    assert!(
        !bytes.contains(TASK_ID),
        "the embedded task id must not reach the relay: {bytes}"
    );
    server.stop();
}

/// The relay's stored row for a sealed explicit-edge push contains no
/// relationship label, and its identifier column is an opaque handle rather than
/// the edge id that embeds the label.
#[test]
fn a_sealed_explicit_edge_push_reaches_the_relay_with_no_relationship_label() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-edge-key")],
    );
    seed_explicit_edge(&a, "bp", RELATIONSHIP);
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    let edge = rows
        .iter()
        .find(|r| r.entity_kind == "assoc_edge")
        .expect("the edge row reaches the relay");
    let bytes = stored_bytes(edge);
    assert!(edge.client_encrypted, "the edge row is sealed");
    assert!(
        !bytes.contains(RELATIONSHIP),
        "the relationship label must not reach the relay: {bytes}"
    );
    assert!(
        !edge.entity_id.contains('\u{1f}'),
        "the identifier column is an opaque handle, not the delimited edge id: {}",
        edge.entity_id
    );
    server.stop();
}

/// The relay cannot confirm a guessed caller-chosen identifier by hashing it.
///
/// A substring assertion cannot see this leak: the stored value is a digest,
/// not the string. So for the two kinds whose whole idempotency-key preimage a
/// journal holder already knows — the persona attribute name beside the stored
/// transaction stamp, and the relationship label inside the edge id beside the
/// stored stamp — this recomputes the published unkeyed hash of the guess and
/// asserts it is not the stored key. A keyed idempotency key makes both
/// recomputations miss.
#[test]
fn a_guessed_identifier_confirms_nothing_against_the_stored_idempotency_key() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-oracle-key")],
    );
    seed_persona(&a, "bp");
    seed_explicit_edge(&a, "bp", RELATIONSHIP);
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv
        .journal_list_since("bp", 0, 100)
        .expect("server journal");
    let row_of = |want: &str| {
        rows.iter()
            .find(|r| r.entity_kind == want)
            .unwrap_or_else(|| panic!("a {want} row reaches the relay"))
    };

    // Persona: the attribute name is the identifier the key names, and the stamp
    // in the preimage is the transaction time the row already carries.
    for row in rows.iter().filter(|r| r.entity_kind == "persona") {
        for guess in [ATTR_NAME, PREF_NAME] {
            let oracle = clio_store::sha256_hex(
                b"am-sync-mut",
                &format!("{guess}|{}|upsert", row.transaction_time),
            );
            assert_ne!(
                row.sync_mutation_id, oracle,
                "a guessed attribute name must not confirm against the stored key"
            );
        }
    }

    // The explicit edge: the preimage is the edge id, whose last component is the
    // relationship label, beside the stored stamp and the row's own op label.
    let edge = row_of("assoc_edge");
    let edge_id = clio_store::explicit_id("bp", "i-1", "i-2", RELATIONSHIP);
    let oracle = clio_store::sha256_hex(
        b"am-sync-mut",
        &format!("{edge_id}|{}|assoc", edge.transaction_time),
    );
    assert_ne!(
        edge.sync_mutation_id, oracle,
        "a guessed relationship label must not confirm against the stored key"
    );

    // The digest column is already keyed; assert the same shape for it so a
    // future change to either value is caught by this one test.
    let persona = row_of("persona");
    assert_ne!(
        persona.content_hash,
        clio_store::sha256_hex(b"am-sync-hash", ATTR_NAME),
        "and the stored digest is not the hash of a guessed value either"
    );
    server.stop();
}

/// With no cipher configured the stored idempotency key is the unkeyed hash of
/// the real identifier a plaintext deployment has always stored, so every
/// existing redelivery check against a stored row still matches.
#[test]
fn plaintext_mode_stores_the_plain_idempotency_key() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    seed_persona(&a, "bp");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    for row in rows.iter().filter(|r| r.entity_kind == "persona") {
        let key_is_attr = row.sync_mutation_id
            == clio_store::sha256_hex(
                b"am-sync-mut",
                &format!("{}|{}|upsert", row.entity_id, row.transaction_time),
            );
        let key_is_pref = row.sync_mutation_id
            == clio_store::sha256_hex(
                b"am-sync-mut",
                &format!("{}|{}|upsert", PREF_NAME, row.transaction_time),
            );
        assert!(
            key_is_attr || key_is_pref,
            "plaintext mode stores the unkeyed hash of the real identifier: {}",
            row.sync_mutation_id
        );
    }
    server.stop();
}

/// A `belief` row's identifier reaches the relay unchanged even with a cipher
/// configured, because a belief id is machine-generated and carries nothing the
/// relay did not already know.
#[test]
fn a_sealed_belief_push_keeps_its_identifier() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("blind-belief-key")],
    );
    a.create_belief(&belief("bel-machine-1", "bp"), "seed")
        .expect("belief");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 10).expect("server journal");
    let row = rows
        .iter()
        .find(|r| r.entity_kind == "belief")
        .expect("the belief row reaches the relay");
    assert_eq!(
        row.entity_id, "bel-machine-1",
        "a machine-generated identifier is not turned into a handle"
    );
    assert!(row.client_encrypted, "the content is still sealed");
    server.stop();
}

/// After an encrypted push of all four kinds the relay's stored journal carries
/// no receipt, no score, and no admitted flag anywhere, while the score is
/// present inside the sealed bytes for the score-bearing kinds. This is the
/// journal-level complement to the pull-path assertions in
/// `client_receipt_tests`.
#[test]
fn an_encrypted_push_leaves_no_score_anywhere_on_the_relay() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let key = SyncCipherKey::from_secret("receipt-journal-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    seed_task_and_failure(&a, "br");
    seed_explicit_edge(&a, "br", RELATIONSHIP);
    push(&cfg_a, &["br".to_owned()]).expect("push");

    let rows = srv.journal_list_since("br", 0, 50).expect("relay journal");
    for kind in ["item", "task", "failure", "assoc_edge"] {
        let row = rows
            .iter()
            .find(|row| row.entity_kind == kind)
            .unwrap_or_else(|| panic!("a {kind} row reaches the relay"));
        assert!(row.client_encrypted, "the {kind} row is sealed");
        assert!(
            row.admission_receipt.is_none(),
            "the relay's {kind} row carries no receipt: {:?}",
            row.admission_receipt
        );
        let bytes = stored_bytes(row);
        assert!(
            !bytes.contains("score"),
            "{kind}: the stored bytes mention a score: {bytes}"
        );
        assert!(
            !bytes.contains("admitted"),
            "{kind}: the stored bytes carry the admitted flag"
        );
        assert!(
            !bytes.contains("clio_admission_receipt"),
            "{kind}: the stored bytes name the receipt key"
        );
    }

    // Positive control: the score is inside the sealed bytes for the
    // score-bearing kinds, so the absence above is the seal working and not
    // the receipt being dropped.
    for kind in ["item", "task", "failure"] {
        let row = rows
            .iter()
            .find(|row| row.entity_kind == kind)
            .expect("the kind reaches the relay");
        let sealed = row
            .content_sealed
            .as_deref()
            .expect("a sealed row stores its envelope");
        let envelope: SyncMutation = serde_json::from_str(sealed).expect("envelope");
        let SyncContent::Cipher(cipher) = envelope.content else {
            panic!("{kind}: the stored envelope is ciphertext");
        };
        let document: Value =
            serde_json::from_slice(&key.open(&cipher).expect("open")).expect("document");
        assert_eq!(
            document[crate::client_feed::package::ADMISSION_RECEIPT_KEY]["admitted"],
            true,
            "{kind}: the score is sealed inside the content"
        );
    }
    server.stop();
}
