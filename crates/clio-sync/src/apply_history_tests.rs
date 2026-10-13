// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the history sync apply effects at the engine level:
//! task/failure payload decoding, bank/erasure checks, last-writer-wins by the
//! task version / failure attempt, idempotent redelivery, and the envelope
//! entity-id binding.
//!
//! ## Owns
//! - Behavior coverage for `apply_history` (task / failure apply effects)
//!
//! ## Does not own
//! - Persona / belief apply effects (`apply_records_tests`)
//! - Push-side packaging / TCP replication (`client_feed_tests`)
//!
//! ## Boundary
//!
//! Tests here break when history apply semantics change; they never change it.
//!
//! Keep this module focused on history-record apply tests.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, fixture};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{HistoryStore, Store, SyncStore};
use clio_types::{
    EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem, SourceType, TaskRecord,
};
use serde_json::{Value, json};

fn mut_for(mid: &str, kind: &str, entity: &str, stamp: &str, content: Value) -> SyncMutation {
    let content_hash = clio_store::sha256_hex(
        crate::keyed_roles::published_digest_salt(),
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

#[test]
fn task_records_apply_versioned_lww_and_duplicate() {
    let (store, _kms) = fixture();
    let v1 = mut_for(
        "m-t1",
        "task",
        "task-1",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-t1", "task-1", 1),
    );
    assert_eq!(apply(&store, None, &v1), ApplyOutcome::Applied);
    let got = store.get_task("b-hist", "task-1").unwrap().expect("task");
    assert_eq!(got.version, 1);
    assert_eq!(got.context.as_deref(), Some("task ctx"));
    // Re-delivery of the same mutation is idempotent (no version bump).
    assert_eq!(apply(&store, None, &v1), ApplyOutcome::Duplicate);
    assert_eq!(
        store.get_task("b-hist", "task-1").unwrap().unwrap().version,
        1
    );
    // A strictly newer version applies.
    let v2 = mut_for(
        "m-t2",
        "task",
        "task-1",
        "2026-01-02T00:00:00Z",
        task_payload("b-hist", "car-t2", "task-1", 2),
    );
    assert_eq!(apply(&store, None, &v2), ApplyOutcome::Applied);
    assert_eq!(
        store.get_task("b-hist", "task-1").unwrap().unwrap().version,
        2
    );
    // A stale version loses and is journaled.
    let v0 = mut_for(
        "m-t0",
        "task",
        "task-1",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-t0", "task-1", 1),
    );
    assert_eq!(apply(&store, None, &v0), ApplyOutcome::ConflictLoser);
    assert_eq!(store.conflict_count("b-hist").unwrap(), 1);
    // Envelope identity binding: entity_id must be the task id.
    let wrong = mut_for(
        "m-tw",
        "task",
        "other-task",
        "2026-01-03T00:00:00Z",
        task_payload("b-hist", "car-tw", "task-1", 3),
    );
    assert_eq!(
        apply(&store, None, &wrong),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn failure_records_apply_slot_lww_and_duplicate() {
    let (store, _kms) = fixture();
    let f1 = mut_for(
        "m-f1",
        "failure",
        "frec-1",
        "2026-01-01T00:00:00Z",
        failure_payload("b-hist", "car-f1", "frec-1", "task-1"),
    );
    assert_eq!(apply(&store, None, &f1), ApplyOutcome::Applied);
    let rows = store.list_failures("b-hist", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].what_failed, "boom");
    assert_eq!(rows[0].context.as_deref(), Some("fail ctx"));
    // Idempotent redelivery.
    assert_eq!(apply(&store, None, &f1), ApplyOutcome::Duplicate);
    assert_eq!(store.list_failures("b-hist", 10).unwrap().len(), 1);
    // A different record id in the same attempt slot loses and is journaled.
    let clash = mut_for(
        "m-f2",
        "failure",
        "frec-other",
        "2026-01-02T00:00:00Z",
        failure_payload("b-hist", "car-f2", "frec-other", "task-1"),
    );
    assert_eq!(apply(&store, None, &clash), ApplyOutcome::ConflictLoser);
    assert_eq!(store.conflict_count("b-hist").unwrap(), 1);
    assert_eq!(store.list_failures("b-hist", 10).unwrap().len(), 1);
}

#[test]
fn history_records_shredded_subject_dead_letter() {
    let (store, _kms) = fixture();
    store
        .upsert_subject_key_ref("s1", "shredded", "shredded")
        .expect("shred marker");
    let task = mut_for(
        "m-ts",
        "task",
        "task-1",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-ts", "task-1", 1),
    );
    assert_eq!(
        apply(&store, None, &task),
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    let failure = mut_for(
        "m-fs",
        "failure",
        "frec-s",
        "2026-01-01T00:00:00Z",
        failure_payload("b-hist", "car-fs", "frec-s", "task-1"),
    );
    assert_eq!(
        apply(&store, None, &failure),
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
}

#[test]
fn task_and_failure_apply_bind_record_context_to_carrier() {
    let (store, _kms) = fixture();
    // A wire record carrying its own (divergent) context is bound to the
    // carrier item's applied context instead of being rejected by the store,
    // which keeps the context on the carrier.
    let task_item = carrier("b-hist", "car-ctx", EpisodicType::Task, "task ctx");
    let task_record = TaskRecord {
        id: "rec-task-ctx-1".into(),
        bank_id: "b-hist".into(),
        item_id: "car-ctx".into(),
        task_id: "task-ctx".into(),
        subject_id: "s1".into(),
        version: 1,
        definition: "do the thing".into(),
        steps: vec!["step".into()],
        status: "open".into(),
        evidence_ref: None,
        context: Some("record ctx".into()),
        admission_score: 0.5,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let m = mut_for(
        "m-ctx",
        "task",
        "task-ctx",
        "2026-01-01T00:00:00Z",
        json!({
            "item": serde_json::to_value(&task_item).expect("item"),
            "task": serde_json::to_value(&task_record).expect("task"),
        }),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let got = store.get_task("b-hist", "task-ctx").unwrap().expect("task");
    assert_eq!(got.context.as_deref(), Some("task ctx"));

    let failure_item = carrier("b-hist", "car-fctx", EpisodicType::Failure, "fail ctx");
    let failure_record = FailureRecord {
        id: "frec-ctx".into(),
        bank_id: "b-hist".into(),
        item_id: "car-fctx".into(),
        task_id: "task-ctx".into(),
        subject_id: "s1".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "don't".into(),
        lesson_tokens: 1,
        evidence_ref: None,
        context: Some("record fail ctx".into()),
        admission_score: 0.5,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let mf = mut_for(
        "m-fctx",
        "failure",
        "frec-ctx",
        "2026-01-01T00:00:00Z",
        json!({
            "item": serde_json::to_value(&failure_item).expect("item"),
            "failure": serde_json::to_value(&failure_record).expect("failure"),
        }),
    );
    assert_eq!(apply(&store, None, &mf), ApplyOutcome::Applied);
    let gotf = store.list_failures("b-hist", 10).unwrap();
    assert_eq!(gotf.len(), 1);
    assert_eq!(gotf[0].context.as_deref(), Some("fail ctx"));
}
