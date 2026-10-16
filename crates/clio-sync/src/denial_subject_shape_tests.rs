// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the subject-keyed payload shapes the denial guard reads a
//! subject from: the belief record's top-level `subject_id`, the episodic
//! carrier item that task and failure records ride on, and the guard's
//! dead-letter or applied outcome for each shape.
//!
//! ## Owns
//! - The belief, task, and failure wire-shape builders for the guard's cases
//! - The refused-shape and clear-subject assertions over those shapes
//!
//! ## Does not own
//! - The denial wire vocabulary itself (`denial_tests`): carrier, proof,
//!   binding, marker, digest
//! - The guard transitions at the engine level (`apply_denial_tests`)
//!
//! ## Boundary
//!
//! Changes to how a payload carries its guarded subject should normally require
//! changes here. Changes to the denial wire vocabulary should not.
//!
//! Keep this module focused on the subject-keyed payload shapes the guard reads.

use crate::apply::apply_tests::{env, fixture};
use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation_guarded};
use crate::client_feed_package_fixtures::{
    belief, failure_record, item as package_item, task_record,
};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{
    ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_TASK, OP_UPSERT, SyncMutation,
};
use clio_store::{HistoryStore, SqliteStore, Store, SyncStore};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem, SourceType};
use serde_json::{Value, json};

/// The belief wire shape the packaging emits: the belief record itself, whose
/// top-level `subject_id` is the guarded field.
fn belief_content(bank: &str, belief_id: &str, subject: &str) -> Value {
    let mut belief = belief(belief_id);
    belief.bank_id = bank.into();
    belief.subject_id = subject.into();
    serde_json::to_value(belief).expect("serialize belief")
}

/// The episodic carrier item a task or failure record rides on, scoped to
/// `bank` and `subject`; the guarded subject is its `subject_id`.
fn carrier_item(bank: &str, item_id: &str, subject: &str, episodic: EpisodicType) -> Value {
    let failure = episodic == EpisodicType::Failure;
    let item = MemoryItem {
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Episodic,
        episodic_type: Some(episodic),
        epistemic_kind: if failure {
            EpistemicKind::Belief
        } else {
            EpistemicKind::Fact
        },
        source_type: failure.then_some(SourceType::AgentInferred),
        ..package_item(item_id)
    };
    serde_json::to_value(item).expect("serialize carrier item")
}

/// The task wire shape the packaging emits: the carrier item plus the
/// structured record.
fn task_content(bank: &str, item_id: &str, task_id: &str, subject: &str) -> Value {
    let mut task = task_record(task_id, "rec-task-1");
    task.bank_id = bank.into();
    task.item_id = item_id.into();
    task.subject_id = subject.into();
    json!({
        "item": carrier_item(bank, item_id, subject, EpisodicType::Task),
        "task": serde_json::to_value(task).expect("serialize task"),
    })
}

/// The failure wire shape, with the same carrier-item subject rule.
fn failure_content(bank: &str, item_id: &str, failure_id: &str, subject: &str) -> Value {
    let mut failure = failure_record(failure_id, "task-1");
    failure.bank_id = bank.into();
    failure.item_id = item_id.into();
    failure.subject_id = subject.into();
    json!({
        "item": carrier_item(bank, item_id, subject, EpisodicType::Failure),
        "failure": serde_json::to_value(failure).expect("serialize failure"),
    })
}

/// One subject-keyed upsert of `kind` carrying `content`, applied with
/// `denial` wired exactly as the routes apply it.
fn shape_op(mid: &str, bank: &str, kind: &str, entity: &str, content: Value) -> SyncMutation {
    env(
        mid,
        bank,
        kind,
        entity,
        OP_UPSERT,
        "2026-01-01T00:00:01Z",
        "dev-a",
        content,
    )
}

/// Apply `m` and assert the denied subject is dead-lettered with no journal
/// entry, the shape-independent half of every refused-shape case.
fn assert_refused(store: &SqliteStore, denial: &FakeDenial, m: &SyncMutation) {
    let engine = ApplyEngine {
        store,
        sync: store,
        device_id: "srv-1".into(),
        kms: None,
    };
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    let result = apply_mutation_guarded(&engine, Some(denial), m, &envelope).expect("apply runs");
    assert_eq!(
        result.outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    assert!(
        store
            .journal_get(&m.bank, &m.sync_mutation_id)
            .unwrap()
            .is_none(),
        "a refused mutation is never journaled"
    );
}

#[test]
fn a_denied_subject_belief_is_dead_lettered_before_any_write() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-belief"]);
    let content = belief_content("b-belief", "belief-1", "s-held");
    let m = shape_op(
        "m-belief",
        "b-belief",
        ENTITY_KIND_BELIEF,
        "belief-1",
        content,
    );
    assert_refused(store.as_ref(), denial.as_ref(), &m);
    assert!(
        store.belief_get("b-belief", "belief-1").unwrap().is_none(),
        "no belief row behind a refused subject"
    );
}

#[test]
fn a_denied_subject_task_is_dead_lettered_before_any_write() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-task"]);
    let content = task_content("b-task", "car-task", "task-1", "s-held");
    let m = shape_op("m-task", "b-task", ENTITY_KIND_TASK, "task-1", content);
    assert_refused(store.as_ref(), denial.as_ref(), &m);
    assert!(
        store
            .get_memory_item("b-task", "car-task")
            .unwrap()
            .is_none(),
        "no carrier item minted behind a refused subject"
    );
    assert!(store.get_task("b-task", "task-1").unwrap().is_none());
}

#[test]
fn a_denied_subject_failure_is_dead_lettered_before_any_write() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-failure"]);
    let content = failure_content("b-failure", "car-failure", "frec-1", "s-held");
    let m = shape_op(
        "m-failure",
        "b-failure",
        ENTITY_KIND_FAILURE,
        "frec-1",
        content,
    );
    assert_refused(store.as_ref(), denial.as_ref(), &m);
    assert!(
        store
            .get_memory_item("b-failure", "car-failure")
            .unwrap()
            .is_none(),
        "no carrier item minted behind a refused subject"
    );
    assert_eq!(store.list_failures("b-failure", 10).unwrap().len(), 0);
}

#[test]
fn a_clear_subject_passes_the_guard_for_every_subject_keyed_shape() {
    let (store, _kms) = fixture();
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-shape"]);
    let engine = ApplyEngine {
        store: store.as_ref(),
        sync: store.as_ref(),
        device_id: "srv-1".into(),
        kms: None,
    };
    let cases = [
        (
            "m-clear-belief",
            ENTITY_KIND_BELIEF,
            "belief-1",
            belief_content("b-shape", "belief-1", "s-clear"),
        ),
        (
            "m-clear-task",
            ENTITY_KIND_TASK,
            "task-1",
            task_content("b-shape", "car-task", "task-1", "s-clear"),
        ),
        (
            "m-clear-failure",
            ENTITY_KIND_FAILURE,
            "frec-1",
            failure_content("b-shape", "car-failure", "frec-1", "s-clear"),
        ),
    ];
    for (mid, kind, entity, content) in cases {
        let m = shape_op(mid, "b-shape", kind, entity, content);
        let envelope = serde_json::to_string(&m).expect("serialize envelope");
        let result = apply_mutation_guarded(&engine, Some(denial.as_ref()), &m, &envelope)
            .expect("apply runs");
        assert_eq!(
            result.outcome,
            ApplyOutcome::Applied,
            "a clear subject must pass the guard for {kind}"
        );
    }
    assert!(store.belief_get("b-shape", "belief-1").unwrap().is_some());
    assert!(store.get_task("b-shape", "task-1").unwrap().is_some());
    assert_eq!(store.list_failures("b-shape", 10).unwrap().len(), 1);
}
