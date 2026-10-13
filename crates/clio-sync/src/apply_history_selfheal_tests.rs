// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the history-apply remedies: the journal self-heal
//! on equal-state task/failure redeliveries (Applied + journaled, relayable
//! to a third peer), the carrier-context divergence detection in `task_equal`,
//! and the payload bank-mismatch fail-closed checks.
//!
//! ## Owns
//! - Regression coverage for the history apply journal self-heal,
//!   equal-version context divergence, and bank-mismatch dead-letters
//!
//! ## Does not own
//! - The happy-path version/attempt ordering matrix (`apply_history_tests`)
//! - Persona / belief self-heal coverage (`apply_records_selfheal_tests`)
//!
//! ## Boundary
//!
//! Tests here break when history apply self-heal semantics change; they never
//! change it.
//!
//! Keep this module focused on history-record self-heal tests.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, fixture};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{HistoryStore, SyncStore};
use clio_types::{
    EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem, SourceType, TaskRecord,
};
use serde_json::{Value, json};

fn mut_for(mid: &str, kind: &str, entity: &str, stamp: &str, content: Value) -> SyncMutation {
    let content_hash = clio_store::sha256_hex(
        crate::keyed_roles::PUBLISHED_DIGEST_SALT,
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

fn carrier(bank: &str, item_id: &str, episodic: EpisodicType, context: Option<&str>) -> MemoryItem {
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
        context: context.map(str::to_owned),
    }
}

fn task_payload(
    bank: &str,
    item_id: &str,
    item_context: Option<&str>,
    task_id: &str,
    version: i64,
) -> Value {
    let item = carrier(bank, item_id, EpisodicType::Task, item_context);
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
    let item = carrier(bank, item_id, EpisodicType::Failure, Some("fail ctx"));
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

/// Journal self-heal: an equal-version task redelivery under a fresh,
/// never-journaled mutation id returns Applied (not Duplicate) so the engine
/// journals the mutation and keeps it relayable to a third peer via pull.
#[test]
fn task_equal_version_fresh_id_applies_and_journals() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-tbase",
        "task",
        "task-1",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-base", Some("task ctx"), "task-1", 1),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);
    // Same version, identical record and carrier context, fresh id + item:
    // the payload content hash differs, so the content guard misses and the
    // equal-state branch must self-heal the journal.
    let dup = mut_for(
        "m-tdup",
        "task",
        "task-1",
        "2026-01-02T00:00:00Z",
        task_payload("b-hist", "car-dup", Some("task ctx"), "task-1", 1),
    );
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    assert!(store.journal_get("b-hist", "m-tdup").unwrap().is_some());
    assert!(store.journal_get("b-hist", "m-tbase").unwrap().is_some());
    assert_eq!(store.conflict_count("b-hist").unwrap(), 0);
    assert_eq!(
        store.get_task("b-hist", "task-1").unwrap().unwrap().version,
        1
    );
}

/// An absent incoming carrier context is no-difference (preserve-on-absent):
/// the equal-state branch still self-heals, and the stored context stays.
#[test]
fn task_equal_version_absent_carrier_context_is_no_difference() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-tbase2",
        "task",
        "task-2",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-b2", Some("task ctx"), "task-2", 1),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);
    let legacy = task_payload("b-hist", "car-legacy", None, "task-2", 1);
    assert!(legacy["item"].get("context").is_none());
    let m = mut_for(
        "m-tlegacy",
        "task",
        "task-2",
        "2026-01-02T00:00:00Z",
        legacy,
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert!(store.journal_get("b-hist", "m-tlegacy").unwrap().is_some());
    let got = store.get_task("b-hist", "task-2").unwrap().expect("task");
    assert_eq!(got.context.as_deref(), Some("task ctx"));
}

/// Item (3): an equal-version task whose carrier context changed is a
/// journaled conflict loser — the context change is never silently dropped.
#[test]
fn task_equal_version_context_change_is_a_journaled_conflict_loser() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-tbase3",
        "task",
        "task-3",
        "2026-01-01T00:00:00Z",
        task_payload("b-hist", "car-b3", Some("task ctx"), "task-3", 1),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);
    let moved = mut_for(
        "m-tmoved",
        "task",
        "task-3",
        "2026-01-02T00:00:00Z",
        task_payload("b-hist", "car-moved", Some("changed ctx"), "task-3", 1),
    );
    assert_eq!(apply(&store, None, &moved), ApplyOutcome::ConflictLoser);
    let got = store.get_task("b-hist", "task-3").unwrap().expect("task");
    assert_eq!(got.context.as_deref(), Some("task ctx"));
    let conflicts = store.conflict_list("b-hist", 10).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].loser_mutation_id, "m-tmoved");
    assert!(store.journal_get("b-hist", "m-tmoved").unwrap().is_none());
}

/// item (2): a task payload whose own carrier-item or record `bank_id`
/// differs from the mutation envelope bank fail-closes to the quarantine.
#[test]
fn task_bank_mismatch_dead_letters() {
    let (store, _kms) = fixture();
    let foreign_item = task_payload("b-other", "car-fb", Some("task ctx"), "task-x", 1);
    let m = mut_for(
        "m-tfb",
        "task",
        "task-x",
        "2026-01-01T00:00:00Z",
        foreign_item,
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    assert!(store.get_task("b-hist", "task-x").unwrap().is_none());
    // Record-level bank mismatch with a matching carrier item.
    let mut foreign_record = task_payload("b-hist", "car-fr", Some("task ctx"), "task-y", 1);
    foreign_record["task"]["bank_id"] = json!("b-other");
    let mr = mut_for(
        "m-tfr",
        "task",
        "task-y",
        "2026-01-01T00:00:00Z",
        foreign_record,
    );
    assert_eq!(
        apply(&store, None, &mr),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    assert!(store.get_task("b-hist", "task-y").unwrap().is_none());
}

/// Journal self-heal: a same-slot failure redelivery under a fresh,
/// never-journaled mutation id returns Applied and journals the mutation.
#[test]
fn failure_same_slot_fresh_id_applies_and_journals() {
    let (store, _kms) = fixture();
    let base = mut_for(
        "m-fbase",
        "failure",
        "frec-1",
        "2026-01-01T00:00:00Z",
        failure_payload("b-hist", "car-fbase", "frec-1", "task-f"),
    );
    assert_eq!(apply(&store, None, &base), ApplyOutcome::Applied);
    let dup = mut_for(
        "m-fdup",
        "failure",
        "frec-1",
        "2026-01-02T00:00:00Z",
        failure_payload("b-hist", "car-fdup", "frec-1", "task-f"),
    );
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    assert!(store.journal_get("b-hist", "m-fdup").unwrap().is_some());
    assert_eq!(store.conflict_count("b-hist").unwrap(), 0);
    assert_eq!(store.list_failures("b-hist", 10).unwrap().len(), 1);
}

/// item (2): a failure payload whose own record `bank_id` differs from
/// the mutation envelope bank fail-closes to the quarantine.
#[test]
fn failure_bank_mismatch_dead_letters() {
    let (store, _kms) = fixture();
    let mut foreign = failure_payload("b-hist", "car-ffb", "frec-ffb", "task-fb");
    foreign["failure"]["bank_id"] = json!("b-other");
    let m = mut_for(
        "m-ffb",
        "failure",
        "frec-ffb",
        "2026-01-01T00:00:00Z",
        foreign,
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    let failures = store.list_failures("b-hist", 10).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}
