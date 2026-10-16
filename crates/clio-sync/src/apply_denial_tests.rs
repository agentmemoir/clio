// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the guarded apply entry at the engine level (SQLite
//! in-memory, no transport): the subject guard ahead of any key mint or
//! content write, the unreadable-authority refusal, the durable record and
//! proof one denial operation produces, its idempotent redelivery, and the
//! unchanged behavior when no authority is wired.
//!
//! ## Owns
//! - Behavior coverage for the denial guards over `apply`
//!
//! ## Does not own
//! - The wire vocabulary in isolation (`denial_tests`)
//! - The routes that carry the proof (`server_denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the apply-side denial guards change; they never
//! change them.
//!
//! Keep this module focused on the guarded apply entry.

use crate::apply::apply_tests::{env, fixture, item_value};
use crate::apply::{ApplyEngine, ApplyOutcome, ApplyResult, apply_mutation_guarded};
use crate::denial::{DenialCarrier, bank_verdict, denial_mutation, denied_bank_marker};
use crate::denial_fixtures::{FakeDenial, store_with_counting_kms};
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_store::{DeadLetterStatus, DenialVerdict, ErasureDenial, SqliteStore, Store, SyncStore};
use clio_types::{AmError, ErrorCode};
use serde_json::json;

/// Apply one mutation with `denial` wired, exactly as the push route does: a
/// fresh engine over the same durable store every call.
fn apply_guarded(
    store: &SqliteStore,
    denial: Option<&dyn ErasureDenial>,
    m: &SyncMutation,
) -> Result<ApplyResult, AmError> {
    let engine = ApplyEngine {
        store,
        sync: store,
        device_id: "srv-1".into(),
        kms: None,
    };
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    apply_mutation_guarded(&engine, denial, m, &envelope)
}

/// The one denial mutation one request produces, riding `bank`.
fn denial_op(bank: &str, subject: &str, request_ref: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        subject,
        &DenialCarrier::new(request_ref, banks),
        "dev-a",
        "2026-01-01T00:00:00Z",
    )
}

/// An item create for one subject in one bank.
fn item_op(mid: &str, bank: &str, item_id: &str, subject: &str, gist: &str) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        item_id,
        OP_UPSERT,
        "2026-01-01T00:00:01Z",
        "dev-a",
        item_value(bank, item_id, subject, "2026-01-01T00:00:01Z", gist),
    )
}

#[test]
fn a_denied_subject_create_mints_nothing() {
    let (store, kms) = store_with_counting_kms();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-held"]);
    let m = item_op("m-denied", "b-held", "i-1", "s-held", "held");
    let result = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("apply runs");
    assert_eq!(
        result.outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    assert!(
        store.get_memory_item("b-held", "i-1").unwrap().is_none(),
        "a denied create writes no content"
    );
    assert!(
        !kms.minted_for("s-held"),
        "no key mint for the denied subject"
    );
    assert!(
        !kms.sealed_for("s-held"),
        "no content seal for the denied subject"
    );
}

#[test]
fn a_clear_subject_still_applies() {
    let (store, kms) = store_with_counting_kms();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-held"]);
    let m = item_op("m-clear", "b-clear", "i-2", "s-clear", "clear");
    let result = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("apply runs");
    assert_eq!(result.outcome, ApplyOutcome::Applied);
    assert!(result.denial_proof.is_none(), "no proof for subject data");
    assert!(store.get_memory_item("b-clear", "i-2").unwrap().is_some());
    assert!(kms.minted_for("s-clear"), "the probe observes a real mint");
    assert!(kms.sealed_for("s-clear"));
}

#[test]
fn an_unreadable_authority_refuses_the_create() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.damage();
    let m = item_op("m-degraded", "b-clear", "i-3", "s-clear", "g");
    let err = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m)
        .expect_err("an unreadable authority refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(store.get_memory_item("b-clear", "i-3").unwrap().is_none());
    assert!(
        store
            .journal_get("b-clear", "m-degraded")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_recorded_denial_marks_its_covered_banks_for_serve() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    let m = denial_op("b-a", "s-1", "req-1", &["b-a", "b-b"]);
    let result = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("apply runs");
    assert_eq!(result.outcome, ApplyOutcome::Applied);
    assert_eq!(bank_verdict(denial.as_ref(), "b-a"), DenialVerdict::Denied);
    assert_eq!(bank_verdict(denial.as_ref(), "b-b"), DenialVerdict::Denied);
    assert_eq!(bank_verdict(denial.as_ref(), "b-c"), DenialVerdict::Clear);
}

#[test]
fn a_redelivered_denial_keeps_one_record_and_the_same_proof() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    let m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    let first = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("first delivery");
    assert_eq!(first.outcome, ApplyOutcome::Applied);
    let proof = first.denial_proof.expect("a durable proof");
    assert_eq!(proof.request_ref, "req-1");
    assert_eq!(
        proof.peer_id, "srv-1",
        "the answering deployment's device id"
    );
    assert_eq!(proof.salt_fingerprint, "fp-1");

    let second = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("redelivery");
    assert_eq!(second.outcome, ApplyOutcome::Duplicate);
    assert_eq!(
        second.denial_proof.expect("the same durable proof"),
        proof,
        "a redelivery repeats no effect but still answers with the proof"
    );
    assert_eq!(denial.records_for("s-1"), 1, "one durable record");
    assert_eq!(store.journal_max_seq("b-a").unwrap(), 1, "one journal row");
}

#[test]
fn a_denied_subject_is_refused_deployment_wide_while_an_unrelated_bank_applies() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    let op = denial_op("b-a", "s-held", "req-1", &["b-a"]);
    apply_guarded(store.as_ref(), Some(denial.as_ref()), &op).expect("record the denial");

    // The recorded denial covers the subject, not one bank: a stale create in
    // another bank would become readable there, so it is refused.
    let stale = item_op("m-stale", "b-other", "i-9", "s-held", "stale");
    let refused = apply_guarded(store.as_ref(), Some(denial.as_ref()), &stale).expect("apply runs");
    assert_eq!(
        refused.outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    assert!(store.get_memory_item("b-other", "i-9").unwrap().is_none());

    let unrelated = item_op("m-fresh", "b-fresh", "i-8", "s-fresh", "fresh");
    let applied =
        apply_guarded(store.as_ref(), Some(denial.as_ref()), &unrelated).expect("apply runs");
    assert_eq!(applied.outcome, ApplyOutcome::Applied);
    assert!(store.get_memory_item("b-fresh", "i-8").unwrap().is_some());
}

#[test]
fn a_malformed_denial_carrier_is_quarantined() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    let mut m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    m.content = SyncContent::Plain(json!({ "request_ref": "", "banks": [] }));
    let result = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("apply runs");
    assert_eq!(
        result.outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    assert_eq!(
        store
            .dead_letter_count("b-a", DeadLetterStatus::Open)
            .unwrap(),
        1
    );
}

#[test]
fn an_unreadable_authority_on_a_denial_leaves_nothing_behind() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.damage();
    let m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    let err = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m)
        .expect_err("an unreadable authority refuses the denial");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        store
            .journal_get("b-a", &m.sync_mutation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .dead_letter_count("b-a", DeadLetterStatus::Open)
            .unwrap(),
        0,
        "a transient refusal quarantines nothing"
    );
}

#[test]
fn an_authority_failure_on_a_redelivery_refuses_instead_of_acknowledging() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    let m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    apply_guarded(store.as_ref(), Some(denial.as_ref()), &m).expect("first delivery");
    denial.damage();
    let err = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m)
        .expect_err("an unreadable authority cannot re-prove");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn a_denial_without_a_wired_authority_keeps_the_unsupported_reject() {
    let (store, _kms) = fixture();
    let m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    let result = apply_guarded(store.as_ref(), None, &m).expect("apply runs");
    assert_eq!(
        result.outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "unsupported_op".into()
        },
        "an unwired deployment behaves exactly as it did before the guards"
    );
}

#[test]
fn subject_data_without_a_wired_authority_is_untouched() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-1", &["b-item"]);
    let m = item_op("m-plain", "b-item", "i-1", "s-1", "plain");
    let result = apply_guarded(store.as_ref(), None, &m).expect("apply runs");
    assert_eq!(result.outcome, ApplyOutcome::Applied);
    assert!(store.get_memory_item("b-item", "i-1").unwrap().is_some());
}

#[test]
fn a_carrier_without_a_salt_fingerprint_refuses_before_any_marker() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("");
    let m = denial_op("b-a", "s-1", "req-1", &["b-a"]);
    let err = apply_guarded(store.as_ref(), Some(denial.as_ref()), &m)
        .expect_err("a carrier whose authority reports no salt fingerprint refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        denial.records_for("s-1"),
        0,
        "no subject record exists without a fingerprint"
    );
    assert_eq!(
        denial.records_for(&denied_bank_marker("b-a")),
        0,
        "no serve marker exists without a fingerprint"
    );
    assert!(
        store
            .journal_get("b-a", &m.sync_mutation_id)
            .unwrap()
            .is_none()
    );
}
