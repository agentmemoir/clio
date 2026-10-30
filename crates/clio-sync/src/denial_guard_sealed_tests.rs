// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the sealed subject guard: which
//! subject a sealed payload resolves to under the serving peer's sync key, and
//! the three-way verdict that resolution produces.
//!
//! ## Owns
//! - The sealed-payload fixtures and the resolution cases
//! - The deny path, the clear control, and the unreadable-authority case
//!
//! ## Does not own
//! - The guard itself (`denial_guard`)
//! - The in-memory denial authority (`denial_fixtures`)
//! - The route that applies the verdict (`server_push`)
//!
//! ## Boundary
//!
//! Changes to what a sealed subject resolves to should normally require changes
//! here. Changes to the authority's own verdicts should not.
//!
//! Keep this module focused on the sealed subject guard.

use crate::client::client_tests::paired_key;
use crate::credential_fixtures::secret;
use crate::crypto::SyncCipherKey;
use crate::denial_fixtures::FakeDenial;
use crate::denial_guard::{sealed_content, sealed_subject_id, sealed_verdict, subject_verdict};
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_store::DenialVerdict;
use serde_json::json;

/// The bank every fixture mutation rides.
const BANK: &str = "sealed-guard";
/// The subject a denial covers in these tests.
const DENIED_SUBJECT: &str = "subject-denied";
/// A subject no denial covers.
const CLEAR_SUBJECT: &str = "subject-clear";

/// The one key this peer serves with: the sync suite's shared paired key,
/// derived once per process, so a case that seals and then serves holds the
/// same key on both sides and no fixture pins key material.
fn key() -> SyncCipherKey {
    paired_key()
}

/// The authority's fingerprint value; the guard never reads it.
fn denial() -> std::sync::Arc<FakeDenial> {
    FakeDenial::new("sealed-guard-fingerprint")
}

/// One item upsert whose content is the payload `content`.
fn item(id: &str, content: serde_json::Value) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: id.to_owned(),
        bank: BANK.to_owned(),
        entity_kind: ENTITY_KIND_ITEM.to_owned(),
        entity_id: id.to_owned(),
        op: OP_UPSERT.to_owned(),
        transaction_time: "2026-01-01T00:00:00Z".to_owned(),
        device_id: "device-a".to_owned(),
        content: SyncContent::Plain(content),
        content_hash: "hash".to_owned(),
        admission_receipt: None,
    }
}

/// The same mutation with its payload sealed under `key`, exactly as the
/// pushing client seals it: the associated data is the mutation's own execution
/// envelope, so the ciphertext is bound to the envelope it travels in.
fn sealed(key: &SyncCipherKey, m: SyncMutation) -> SyncMutation {
    let plain = serde_json::to_vec(m.content_value()).expect("serialize the payload");
    let aad = m.execution_view().aad();
    let cipher = key.seal(&plain, &aad).expect("seal the payload");
    SyncMutation {
        content: SyncContent::Cipher(cipher),
        ..m
    }
}

#[test]
fn a_sealed_item_resolves_the_subject_it_carries() {
    let m = sealed(
        &key(),
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    assert_eq!(
        sealed_subject_id(&key(), &m).as_deref(),
        Some(DENIED_SUBJECT),
        "the serving peer reads the subject out of the sealed payload"
    );
}

#[test]
fn a_sealed_item_for_an_erased_subject_is_denied() {
    let authority = denial();
    authority.deny(DENIED_SUBJECT, &[BANK]);
    let m = sealed(
        &key(),
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    assert!(
        matches!(
            sealed_verdict(authority.as_ref(), &key(), &m),
            DenialVerdict::Denied
        ),
        "a sealed create for an erased subject is denied"
    );
}

#[test]
fn a_sealed_item_opens_the_payload_the_writer_sealed() {
    let m = sealed(
        &key(),
        item("i-clear", json!({ "subject_id": CLEAR_SUBJECT })),
    );
    assert_eq!(
        sealed_content(&key(), &m),
        Some(json!({ "subject_id": CLEAR_SUBJECT })),
        "the apply path receives the payload the writer sealed"
    );
}

#[test]
fn a_sealed_item_for_a_clear_subject_still_applies() {
    let authority = denial();
    authority.deny(DENIED_SUBJECT, &[BANK]);
    let m = sealed(
        &key(),
        item("i-clear", json!({ "subject_id": CLEAR_SUBJECT })),
    );
    assert!(
        matches!(
            sealed_verdict(authority.as_ref(), &key(), &m),
            DenialVerdict::Clear
        ),
        "only the erased subject is refused; an unrelated one is untouched"
    );
}

#[test]
fn an_unreadable_authority_is_never_clear() {
    let authority = denial();
    authority.deny(DENIED_SUBJECT, &[BANK]);
    authority.damage();
    let m = sealed(
        &key(),
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    assert!(
        matches!(
            sealed_verdict(authority.as_ref(), &key(), &m),
            DenialVerdict::AuthorityUnavailable
        ),
        "a damaged authority refuses rather than clearing the subject"
    );
}

#[test]
fn a_payload_another_key_sealed_resolves_nothing() {
    let authority = denial();
    authority.deny(DENIED_SUBJECT, &[BANK]);
    // A second generated secret, tagged apart from the shared key's own, so the
    // two derivations cannot coincide and this peer never holds this one.
    let foreign = SyncCipherKey::from_secret(&secret("a-key-this-peer-never-held"));
    let m = sealed(
        &foreign,
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    assert_eq!(
        sealed_subject_id(&key(), &m),
        None,
        "a ciphertext this peer cannot open resolves no subject"
    );
    assert!(
        matches!(
            sealed_verdict(authority.as_ref(), &key(), &m),
            DenialVerdict::Clear
        ),
        "an unopenable sealed row keeps the behavior it had before the guard"
    );
}

#[test]
fn a_tampered_ciphertext_resolves_nothing() {
    let mut m = sealed(
        &key(),
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    let SyncContent::Cipher(cipher) = &mut m.content else {
        panic!("the fixture payload is sealed");
    };
    cipher.ciphertext_b64.push('A');
    assert_eq!(
        sealed_subject_id(&key(), &m),
        None,
        "a ciphertext that does not authenticate resolves no subject"
    );
}

#[test]
fn a_sealed_payload_without_a_subject_resolves_nothing() {
    let m = sealed(
        &key(),
        item("i-empty", json!({ "title": "no subject here" })),
    );
    assert_eq!(
        sealed_subject_id(&key(), &m),
        None,
        "a payload that carries no subject id resolves nothing"
    );
}

#[test]
fn an_unsealed_payload_is_not_resolved_by_the_sealed_reader() {
    let m = item("i-clear", json!({ "subject_id": CLEAR_SUBJECT }));
    assert_eq!(
        sealed_subject_id(&key(), &m),
        None,
        "plaintext resolution belongs to the plaintext reader"
    );
}

#[test]
fn the_plaintext_reader_still_answers_for_a_sealed_payload() {
    let authority = denial();
    authority.deny(DENIED_SUBJECT, &[BANK]);
    let m = sealed(
        &key(),
        item("i-denied", json!({ "subject_id": DENIED_SUBJECT })),
    );
    assert!(
        matches!(
            subject_verdict(authority.as_ref(), &m.entity_kind, m.content_value()),
            DenialVerdict::Clear
        ),
        "the plaintext reader sees no subject on a sealed row, which is why the sealed guard runs"
    );
}
