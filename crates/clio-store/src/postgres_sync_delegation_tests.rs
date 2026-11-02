// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `SyncStore for PostgresStore` delegation arms that the
//! contract-parity and history suites reach through their own modules: the
//! persona and belief apply primitives, the task and failure apply primitives,
//! the history change feeds, and the bank-level persona erasure probe.
//!
//! ## Owns
//! - The persona, belief, task, failure, and history arms of the
//!   `SyncStore for PostgresStore` impl block
//!
//! ## Does not own
//! - The journal, cursor, conflict, and dead-letter arms (`postgres_sync_tests`)
//! - The high-water mark (`postgres_sync_high_water_tests`)
//! - The SQL bodies behind these arms (`postgres_sync_state`,
//!   `postgres_sync_history`, `postgres_sync_records`)
//!
//! ## Boundary
//!
//! Tests here break when an arm is added to, removed from, or rewired in the
//! Postgres `SyncStore` impl. They never assert SQL semantics the body modules
//! own.
//!
//! Keep this module focused on the Postgres `SyncStore` delegation surface.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, FailureRecord, ItemKind,
    MemoryItem, PersonaPreference, PersonaStableEntry, SourceType, TaskRecord, proposition_key,
};
use std::sync::Arc;

/// Empty-watermark sentinel the client push cursor uses before any push.
const SENTINEL: &str = "1970-01-01T00:00:00Z";

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

fn carrier(bank: &str, item_id: &str, episodic: EpisodicType) -> MemoryItem {
    MemoryItem {
        id: item_id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(episodic),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: Some("gist".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn task_draft(bank: &str, item_id: &str) -> TaskRecord {
    TaskRecord {
        id: unique("task"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: "task-sync".into(),
        subject_id: "subject-1".into(),
        version: 1,
        definition: "fix the login test".into(),
        steps: vec!["run tests".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.7,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

fn failure_draft(bank: &str, item_id: &str) -> FailureRecord {
    FailureRecord {
        id: unique("failure"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: "task-sync".into(),
        subject_id: "subject-1".into(),
        attempt_n: 2,
        what_failed: "assertion error".into(),
        lesson: "read the fixture".into(),
        lesson_tokens: 3,
        evidence_ref: None,
        context: None,
        admission_score: 0.7,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

/// The persona and belief arms delegate to the store primitives and read the
/// same durable rows back.
#[test]
fn pg_delegation_persona_arms_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("delegate-persona");
    let s = open_pg();

    assert!(
        s.persona_stable_current(&bank, "style")
            .expect("current")
            .is_none()
    );
    let entry = PersonaStableEntry {
        bank_id: bank.clone(),
        key: "style".into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: None,
        admission_score: 0.5,
        as_of: "2026-06-01".into(),
        valid_until: None,
    };
    s.persona_stable_put(&entry, "seed").expect("put stable");
    assert_eq!(
        s.persona_stable_current(&bank, "style")
            .expect("current")
            .expect("entry")
            .value,
        "terse"
    );

    assert!(
        s.persona_preference_current(&bank, "brevity")
            .expect("current")
            .is_none()
    );
    let pref = PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: None,
        admission_score: 0.5,
        as_of: "2026-06-01".into(),
    };
    s.persona_preference_put(&pref, "seed").expect("put pref");
    assert_eq!(
        s.persona_preference_current(&bank, "brevity")
            .expect("current")
            .expect("pref")
            .value,
        0.4
    );
}

#[test]
fn pg_delegation_belief_arms_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("delegate-belief");
    let s = open_pg();

    let belief = BeliefObject {
        id: unique("belief"),
        bank_id: bank.clone(),
        subject_id: "subject-1".into(),
        proposition: unique("user prefers oat milk"),
        context: Some("stated twice".into()),
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.7, "ev", SourceType::UserStated).expect("entry"),
        ],
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
    };
    assert!(s.belief_get(&bank, &belief.id).expect("get").is_none());
    s.belief_create(&belief, "seed").expect("create");
    assert_eq!(
        s.belief_get(&bank, &belief.id)
            .expect("get")
            .expect("belief")
            .proposition,
        belief.proposition
    );
    let key = proposition_key(&belief.proposition);
    assert_eq!(
        s.belief_by_proposition(&bank, &key)
            .expect("by proposition")
            .expect("belief")
            .id,
        belief.id
    );

    let entry =
        ConfidenceEntry::new("2026-06-02", 0.8, "ev2", SourceType::UserStated).expect("entry");
    s.belief_append(&bank, &belief.id, &entry, "seed", Some("stated twice"))
        .expect("append");
    assert_eq!(
        s.belief_get(&bank, &belief.id)
            .expect("get")
            .expect("belief")
            .confidence_history
            .len(),
        2
    );

    s.belief_correct_context(&bank, &belief.id, "corrected", "test", "agent")
        .expect("correct");
    assert_eq!(
        s.belief_get(&bank, &belief.id)
            .expect("get")
            .expect("belief")
            .context
            .as_deref(),
        Some("corrected")
    );

    // The bank-level erasure probe answers for its own bank only.
    assert!(!s.persona_context_erased(&bank).expect("probe"));
    assert!(
        !s.persona_context_erased(&unique("delegate-erase"))
            .expect("probe")
    );
}

/// The task and failure arms commit a carrier item plus its structured row, and
/// both history feeds map the committed rows back.
#[test]
fn pg_delegation_history_arms_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("delegate-history");
    let s = open_pg();

    assert!(
        s.feed_tasks(&bank, SENTINEL, 10).expect("feed").is_empty(),
        "a fresh bank has no task rows"
    );
    assert!(
        s.feed_failures(&bank, SENTINEL, 10)
            .expect("feed")
            .is_empty(),
        "a fresh bank has no failure rows"
    );

    let item = carrier(&bank, &unique("task-item"), EpisodicType::Task);
    let stored = s
        .task_put(&item, &task_draft(&bank, &item.id), "sync-apply")
        .expect("task put");
    assert_eq!(stored.version, 1);
    assert_eq!(
        s.task_current(&bank, "task-sync")
            .expect("current")
            .expect("row")
            .id,
        stored.id
    );
    assert!(
        s.task_current(&bank, &unique("task-missing"))
            .expect("current")
            .is_none()
    );
    let tasks = s.feed_tasks(&bank, SENTINEL, 10).expect("feed tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].1.task_id, "task-sync");

    let mut failure_item = carrier(&bank, &unique("failure-item"), EpisodicType::Failure);
    failure_item.epistemic_kind = EpistemicKind::Belief;
    failure_item.source_type = Some(SourceType::AgentInferred);
    let failure = s
        .failure_put(
            &failure_item,
            &failure_draft(&bank, &failure_item.id),
            "sync-apply",
        )
        .expect("failure put");
    assert_eq!(failure.attempt_n, 2);
    assert_eq!(
        s.failure_by_attempt(&bank, "task-sync", 2)
            .expect("by attempt")
            .expect("row")
            .id,
        failure.id
    );
    assert!(
        s.failure_by_attempt(&bank, "task-sync", 9)
            .expect("by attempt")
            .is_none()
    );
    let failures = s.feed_failures(&bank, SENTINEL, 10).expect("feed failures");
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1.attempt_n, 2);
}
