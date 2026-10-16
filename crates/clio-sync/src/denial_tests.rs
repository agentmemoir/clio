// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the denial operation's wire vocabulary in isolation: the
//! stable mutation id, the carrier decode and validation, the proof's
//! fail-closed verification, the reserved bank marker namespace, and the
//! subject digest binding.
//!
//! ## Owns
//! - Unit coverage for `denial`: carrier, proof, binding, marker, digest
//!
//! ## Does not own
//! - The authority-facing guards (`apply_denial_tests`)
//! - The routes that carry the proof (`server_denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the wire vocabulary changes; they never change it.
//!
//! Keep this module focused on the denial wire vocabulary.

use crate::crypto::SyncCipher;
use crate::denial::{
    DenialCarrier, DenialProof, DenialProofBinding, bank_verdict, denial_carrier, denial_mutation,
    denial_mutation_id, denial_subject_hash,
};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{ENTITY_KIND_ITEM, SyncContent, SyncMutation};
use clio_store::DenialVerdict;
use clio_types::ErrorCode;
use serde_json::json;

/// The plaintext denial mutation one request produces.
fn request_mutation(subject: &str, request_ref: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        "b-a",
        subject,
        &DenialCarrier::new(request_ref, banks),
        "peer-1",
        "2026-01-01T00:00:00Z",
    )
}

/// The proof a peer with fingerprint `fp-1` returns for one request.
fn proof(request_ref: &str, subject: &str) -> DenialProof {
    DenialProof {
        request_ref: request_ref.into(),
        peer_id: "peer-1".into(),
        subject_hash: denial_subject_hash("fp-1", subject),
        generation: 3,
        salt_fingerprint: "fp-1".into(),
    }
}

#[test]
fn a_retry_reuses_one_mutation_id_per_request_reference() {
    let first = request_mutation("s-1", "req-1", &["b-a"]);
    let retry = request_mutation("s-1", "req-1", &["b-a"]);
    assert_eq!(
        retry.sync_mutation_id, first.sync_mutation_id,
        "a retry must reuse the idempotency key"
    );
    assert_eq!(first.sync_mutation_id, denial_mutation_id("req-1"));
    let other = request_mutation("s-1", "req-2", &["b-a"]);
    assert_ne!(other.sync_mutation_id, first.sync_mutation_id);
}

#[test]
fn a_denial_mutation_decodes_to_its_request_and_banks() {
    let m = request_mutation("s-1", "req-1", &["b-a", "b-b"]);
    let carrier = denial_carrier(&m)
        .expect("decode succeeds")
        .expect("carrier");
    assert_eq!(carrier.request_ref, "req-1");
    assert_eq!(carrier.banks, vec!["b-a".to_owned(), "b-b".to_owned()]);
    assert_eq!(m.entity_id, "s-1", "the subject rides the entity id");
}

#[test]
fn a_mutation_of_another_kind_is_not_a_carrier() {
    let mut item = request_mutation("s-1", "req-1", &["b-a"]);
    item.entity_kind = ENTITY_KIND_ITEM.into();
    assert!(denial_carrier(&item).expect("decode succeeds").is_none());
    let mut invalidate = request_mutation("s-1", "req-1", &["b-a"]);
    invalidate.op = "discard".into();
    assert!(
        denial_carrier(&invalidate)
            .expect("decode succeeds")
            .is_none()
    );
}

#[test]
fn a_carrier_canonicalizes_its_covered_banks() {
    let carrier = DenialCarrier::new("req-1", &["b-b", "b-a", "b-a"]);
    assert_eq!(
        carrier.covered_banks(),
        vec!["b-a".to_owned(), "b-b".to_owned()],
        "one bank costs one record whatever order the carrier listed it in"
    );
}

#[test]
fn a_carrier_without_a_request_reference_is_refused() {
    let mut m = request_mutation("s-1", "req-1", &["b-a"]);
    m.content = SyncContent::Plain(json!({ "request_ref": "  ", "banks": ["b-a"] }));
    let err = denial_carrier(&m).expect_err("blank request reference");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_carrier_without_a_covered_bank_is_refused() {
    let mut m = request_mutation("s-1", "req-1", &["b-a"]);
    m.content = SyncContent::Plain(json!({ "request_ref": "req-1", "banks": [] }));
    let err = denial_carrier(&m).expect_err("no covered bank");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    m.content = SyncContent::Plain(json!({ "request_ref": "req-1", "banks": [" "] }));
    let err = denial_carrier(&m).expect_err("blank bank");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_ciphertext_carrier_is_refused() {
    let mut m = request_mutation("s-1", "req-1", &["b-a"]);
    m.content = SyncContent::Cipher(SyncCipher {
        alg: "xchacha20poly1305".into(),
        key_id: None,
        nonce_b64: "bm9uY2U".into(),
        ciphertext_b64: "Y3Q".into(),
    });
    let err = denial_carrier(&m).expect_err("a denial must travel readable");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_proof_verifies_against_its_own_binding() {
    let m = request_mutation("s-1", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let binding = DenialProofBinding::for_carrier(&m, &carrier, "peer-1", "fp-1");
    assert_eq!(proof("req-1", "s-1").verify(&binding), Ok(()));
}

#[test]
fn a_proof_answered_under_another_fingerprint_is_refused() {
    let m = request_mutation("s-1", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let elsewhere = DenialProofBinding::for_carrier(&m, &carrier, "peer-1", "fp-2");
    let err = proof("req-1", "s-1")
        .verify(&elsewhere)
        .expect_err("another deployment's fingerprint");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn a_proof_bound_to_another_request_or_peer_is_refused() {
    let m = request_mutation("s-1", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let wrong_peer = DenialProofBinding::for_carrier(&m, &carrier, "peer-2", "fp-1");
    assert!(proof("req-1", "s-1").verify(&wrong_peer).is_err());

    let other = request_mutation("s-1", "req-2", &["b-a"]);
    let other_carrier = denial_carrier(&other).expect("decode").expect("carrier");
    let wrong_request = DenialProofBinding::for_carrier(&other, &other_carrier, "peer-1", "fp-1");
    assert!(proof("req-1", "s-1").verify(&wrong_request).is_err());
}

#[test]
fn a_proof_bound_to_another_subject_is_refused() {
    let m = request_mutation("s-2", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let other_subject = DenialProofBinding::for_carrier(&m, &carrier, "peer-1", "fp-1");
    assert!(proof("req-1", "s-1").verify(&other_subject).is_err());
}

#[test]
fn a_proof_for_another_action_is_refused() {
    let m = request_mutation("s-1", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let binding = DenialProofBinding {
        action: "erasure",
        ..DenialProofBinding::for_carrier(&m, &carrier, "peer-1", "fp-1")
    };
    assert!(proof("req-1", "s-1").verify(&binding).is_err());
}

#[test]
fn a_proof_without_a_recorded_generation_is_refused() {
    let m = request_mutation("s-1", "req-1", &["b-a"]);
    let carrier = denial_carrier(&m).expect("decode").expect("carrier");
    let binding = DenialProofBinding::for_carrier(&m, &carrier, "peer-1", "fp-1");
    let mut empty = proof("req-1", "s-1");
    empty.generation = 0;
    assert!(empty.verify(&binding).is_err());
}

#[test]
fn a_denied_bank_reports_denied_and_unrelated_banks_stay_clear() {
    let denial = FakeDenial::new("fp-1");
    assert_eq!(bank_verdict(denial.as_ref(), "b-a"), DenialVerdict::Clear);
    denial.deny_bank("b-a");
    assert_eq!(bank_verdict(denial.as_ref(), "b-a"), DenialVerdict::Denied);
    assert_eq!(bank_verdict(denial.as_ref(), "b-b"), DenialVerdict::Clear);
}

#[test]
fn a_damaged_authority_reports_authority_unavailable() {
    let denial = FakeDenial::new("fp-1");
    denial.damage();
    assert_eq!(
        bank_verdict(denial.as_ref(), "b-a"),
        DenialVerdict::AuthorityUnavailable,
        "a damaged authority is never a clear bank"
    );
}

#[test]
fn the_subject_digest_is_stable_and_bound_to_the_fingerprint() {
    assert_eq!(
        denial_subject_hash("fp-1", "s-1"),
        denial_subject_hash("fp-1", "s-1")
    );
    assert_ne!(
        denial_subject_hash("fp-1", "s-1"),
        denial_subject_hash("fp-2", "s-1")
    );
    assert_ne!(
        denial_subject_hash("fp-1", "s-1"),
        denial_subject_hash("fp-1", "s-2")
    );
}

#[test]
fn the_fake_authority_matches_the_seam_contract() {
    let denial = FakeDenial::new("fp-1");
    let first = denial.deny("s-1", &["b-a"]);
    let again = denial.deny("s-1", &["b-b"]);
    assert_eq!(first, again, "a redelivery returns the current generation");
    assert_eq!(denial.records_for("s-1"), 1, "one durable record");
    assert_eq!(denial.banks_for("s-1"), Some(vec!["b-a".to_owned()]));
    assert_eq!(denial.fingerprint(), "fp-1");
    assert_eq!(denial.generation(), 1);
}
