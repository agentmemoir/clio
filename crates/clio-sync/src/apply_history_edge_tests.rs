// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the uncovered branches of the history apply effects
//! that the happy-path engine tests do not reach: client-ciphertext no-ops,
//! malformed and structurally invalid payload rejection, the envelope
//! entity-id binding for both records, equal-version task journal self-heal
//! and divergences (the `task_equal` helper), and the same-slot failure
//! self-heal.
//!
//! ## Owns
//! - Edge-branch behavior coverage for `apply_history` (task / failure apply)
//!
//! ## Does not own
//! - The happy-path version/attempt ordering matrix (`apply_history_tests`)
//! - Persona / belief apply effects (`apply_records_tests`)
//!
//! ## Boundary
//!
//! Tests here break when history apply semantics change; they never change it.
//!
//! Keep this module focused on history-record apply edge tests.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, fixture};
use crate::crypto::SyncCipher;
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{HistoryStore, SyncStore};
use clio_types::{
    EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem, SourceType, TaskRecord,
};
use serde_json::{Value, json};

fn mut_for(mid: &str, kind: &str, entity: &str, stamp: &str, content: Value) -> SyncMutation {
    let content_hash = clio_store::sha256_hex(
        crate::keyed_roles::PLAINTEXT_DIGEST_SALT,
        &serde_json::to_string(&content).unwrap_or_default(),
    );
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: "b-hist".into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: "upsert".into(),
        transaction_time: stamp.into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(content),
        content_hash,
        admission_receipt: None,
    }
}

fn cipher_mut(mid: &str, kind: &str, entity: &str) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: "b-hist".into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content: SyncContent::Cipher(SyncCipher {
            alg: "xchacha20poly1305".into(),
            key_id: None,
            nonce_b64: String::new(),
            ciphertext_b64: String::new(),
        }),
        content_hash: format!("cipher-{mid}"),
        admission_receipt: None,
    }
}

fn carrier(bank: &str, item_id: &str, episodic: EpisodicType, context: &str) -> MemoryItem {
    let epistemic = if episodic == EpisodicType::Failure {
        EpistemicKind::Belief
    } else {
        EpistemicKind::Fact
    };
    MemoryItem {
        id: item_id.into(),
        bank_id: bank.into(),
        subject_id: "s1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(episodic),
        epistemic_kind: epistemic,
        source_type: (episodic == EpisodicType::Failure).then_some(SourceType::AgentInferred),
        confidence: None,
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: Some(json!({"k": "v"})),
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: Some(context.into()),
    }
}

fn task_payload(bank: &str, item_id: &str, task_id: &str, version: i64) -> Value {
    let item = carrier(bank, item_id, EpisodicType::Task, "task ctx");
    let task = TaskRecord {
        id: format!("rec-{task_id}-{version}"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: task_id.into(),
        subject_id: "s1".into(),
        version,
        definition: "do the thing".into(),
        steps: vec!["step".into()],
        status: "open".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.5,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    json!({
        "item": serde_json::to_value(item).expect("item"),
        "task": serde_json::to_value(task).expect("task"),
    })
}

fn failure_payload(bank: &str, item_id: &str, failure_id: &str, task_id: &str) -> Value {
    let item = carrier(bank, item_id, EpisodicType::Failure, "fail ctx");
    let failure = FailureRecord {
        id: failure_id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: task_id.into(),
        subject_id: "s1".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "don't".into(),
        lesson_tokens: 1,
        evidence_ref: None,
        context: None,
        admission_score: 0.5,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    json!({
        "item": serde_json::to_value(item).expect("item"),
        "failure": serde_json::to_value(failure).expect("failure"),
    })
}

/// A client-encrypted (`SyncContent::Cipher`) task/failure envelope is a
/// server-blind no-op: the apply reports success without decrypting or
/// materializing any durable row.
#[test]
fn history_apply_cipher_content_is_a_server_blind_noop() {
    let (store, _kms) = fixture();
    assert_eq!(
        apply(
            &store,
            None,
            &cipher_mut("m-cipher-task", "task", "task-cipher")
        ),
        ApplyOutcome::Applied
    );
    assert!(store.get_task("b-hist", "task-cipher").unwrap().is_none());
    assert_eq!(
        apply(
            &store,
            None,
            &cipher_mut("m-cipher-failure", "failure", "frec-cipher")
        ),
        ApplyOutcome::Applied
    );
    let failures = store.list_failures("b-hist", 10).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

/// Malformed and structurally invalid task payloads are dead-lettered with the
/// malformed reason code and never create a task row.
#[test]
fn task_apply_dead_letters_malformed_and_invalid_payloads() {
    let (store, _kms) = fixture();
    let mut bad_record = task_payload("b-hist", "car-br", "task-br", 1);
    bad_record["task"] = json!(42);
    let mut bad_def = task_payload("b-hist", "car-bd", "task-bd", 1);
    bad_def["task"]["definition"] = json!("");
    let mut bad_item = task_payload("b-hist", "car-bi", "task-bi", 1);
    bad_item["item"]["valid_from"] = json!("");
    for (mid, entity, payload) in [
        ("m-br", "task-br", bad_record),
        ("m-bd", "task-bd", bad_def),
        ("m-bi", "task-bi", bad_item),
    ] {
        let m = mut_for(mid, "task", entity, "2026-01-01T00:00:00Z", payload);
        assert_eq!(
            apply(&store, None, &m),
            ApplyOutcome::DeadLettered {
                reason_code: "malformed".into()
            },
            "{entity} must be dead-lettered"
        );
    }
    assert!(store.get_task("b-hist", "task-br").unwrap().is_none());
    assert!(store.get_task("b-hist", "task-bd").unwrap().is_none());
    assert!(store.get_task("b-hist", "task-bi").unwrap().is_none());
}

/// Equal-version task redelivery under a fresh mutation id is a journal
/// self-heal (`Applied` + journaled), an equal-version divergence keeps
/// the local row and journals the peer, and a carrier-item collision is
/// dead-lettered.
#[test]
fn task_apply_equal_version_duplicate_divergence_and_put_error() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-base",
        "task",
        "task-1",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-base", "task-1", 1),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);

    // Same version and identical structured fields, fresh id + carrier item:
    // Journal self-heal — Applied so the mutation is journaled.
    let dup = mut_for(
        "m-dup",
        "task",
        "task-1",
        "2026-01-02T00:00:00Z",
        task_payload("b-hist", "car-dup", "task-1", 1),
    );
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    assert!(store.journal_get("b-hist", "m-dup").unwrap().is_some());

    // Same version, divergent definition: local wins, peer is journaled.
    let mut divergent = task_payload("b-hist", "car-div", "task-1", 1);
    divergent["task"]["definition"] = json!("changed");
    let div = mut_for("m-div", "task", "task-1", "2026-01-02T00:00:00Z", divergent);
    assert_eq!(apply(&store, None, &div), ApplyOutcome::ConflictLoser);
    assert_eq!(
        store
            .get_task("b-hist", "task-1")
            .unwrap()
            .unwrap()
            .definition,
        "do the thing"
    );
    assert_eq!(store.conflict_count("b-hist").unwrap(), 1);

    // A new task id reusing a committed carrier item id fails the atomic
    // history commit and is dead-lettered rather than failing the batch.
    let collide = mut_for(
        "m-collide",
        "task",
        "task-collide",
        "2026-01-03T00:00:00Z",
        task_payload("b-hist", "car-base", "task-collide", 1),
    );
    assert_eq!(
        apply(&store, None, &collide),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    assert!(store.get_task("b-hist", "task-collide").unwrap().is_none());
}

/// Malformed, id-mismatched, and structurally invalid failure payloads are
/// dead-lettered with the malformed reason code and leave no durable row.
#[test]
fn failure_apply_dead_letters_malformed_and_invalid_payloads() {
    let (store, _kms) = fixture();
    let mut bad_record = failure_payload("b-hist", "car-fbr", "frec-fbr", "task-1");
    bad_record["failure"] = json!("nope");
    let mut bad_attempt = failure_payload("b-hist", "car-fba", "frec-fba", "task-1");
    bad_attempt["failure"]["attempt_n"] = json!(0);
    let mut bad_item = failure_payload("b-hist", "car-fbi", "frec-fbi", "task-1");
    bad_item["item"]["valid_from"] = json!("");
    // The envelope entity_id must be the failure id (here it is `frec-other`).
    let mismatched = failure_payload("b-hist", "car-fid", "frec-fid", "task-1");
    for (mid, entity, payload) in [
        ("m-fbr", "frec-fbr", bad_record),
        ("m-fid", "frec-other", mismatched),
        ("m-fba", "frec-fba", bad_attempt),
        ("m-fbi", "frec-fbi", bad_item),
    ] {
        let m = mut_for(mid, "failure", entity, "2026-01-01T00:00:00Z", payload);
        assert_eq!(
            apply(&store, None, &m),
            ApplyOutcome::DeadLettered {
                reason_code: "malformed".into()
            },
            "{entity} must be dead-lettered"
        );
    }
    let failures = store.list_failures("b-hist", 10).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

/// A same-slot failure redelivery (same record id) under a fresh mutation id
/// is a journal self-heal (`Applied` + journaled); a carrier-item
/// collision on a new slot is dead-lettered.
#[test]
fn failure_apply_slot_duplicate_and_put_error() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-f1base",
        "failure",
        "frec-1",
        "2026-01-01T00:00:00Z",
        failure_payload("b-hist", "car-f1base", "frec-1", "task-z"),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);

    let dup = mut_for(
        "m-f1dup",
        "failure",
        "frec-1",
        "2026-01-02T00:00:00Z",
        failure_payload("b-hist", "car-f1dup", "frec-1", "task-z"),
    );
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    assert!(store.journal_get("b-hist", "m-f1dup").unwrap().is_some());

    // New attempt slot, but the carrier item id is already committed.
    let mut collide_payload = failure_payload("b-hist", "car-f1base", "frec-3", "task-z");
    collide_payload["failure"]["attempt_n"] = json!(2);
    let collide = mut_for(
        "m-f1col",
        "failure",
        "frec-3",
        "2026-01-03T00:00:00Z",
        collide_payload,
    );
    assert_eq!(
        apply(&store, None, &collide),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    assert_eq!(store.list_failures("b-hist", 10).unwrap().len(), 1);
}
