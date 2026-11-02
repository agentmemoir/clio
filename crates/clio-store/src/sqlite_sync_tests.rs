// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `SyncStore for SqliteStore` delegation surface: every
//! trait arm on this backend is called once so the impl block itself is
//! exercised, not only the SQL bodies it forwards to.
//!
//! ## Owns
//! - The `SyncStore for SqliteStore` impl block: journal, cursor, high-water,
//!   conflict, dead-letter, erasure probes, change feeds, persona/belief
//!   apply primitives, and the history sync arms
//!
//! ## Does not own
//! - The SQL bodies behind the arms (`sqlite_sync_state_tests`,
//!   `sync_store_tests`, `sync_store_tests_feed_tests`)
//! - Postgres parity (`postgres_sync_state_tests`, `postgres_sync_history_tests`)
//!
//! ## Boundary
//!
//! Tests here break when an arm is added to, removed from, or rewired in the
//! SQLite `SyncStore` impl. They never assert SQL semantics the body modules own.
//!
//! Keep this module focused on the SQLite `SyncStore` delegation surface.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};
use clio_types::{ConfidenceEntry, EpisodicType, EpistemicKind, ItemKind, MemoryItem, SourceType};
use std::sync::Arc;

/// Empty-watermark sentinel the client push cursor uses before any push.
const SENTINEL: &str = "1970-01-01T00:00:00Z";

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn journal_row(id: &str, bank: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: id.into(),
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{id}"),
        op: "upsert".into(),
        transaction_time: "2026-06-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: format!("hash-{id}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
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

fn task_draft(bank: &str, item_id: &str) -> clio_types::TaskRecord {
    clio_types::TaskRecord {
        id: "task-1".into(),
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

fn failure_draft(bank: &str, item_id: &str) -> clio_types::FailureRecord {
    clio_types::FailureRecord {
        id: "failure-1".into(),
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

#[test]
fn journal_cursor_conflict_and_dead_letter_arms_delegate() {
    let s = store();
    let bank = "bank-delegate";

    assert!(
        s.journal_append(&journal_row("m1", bank))
            .expect("append")
            .inserted
    );
    let got = s.journal_get(bank, "m1").expect("get").expect("row");
    assert_eq!(got.sync_mutation_id, "m1");
    assert_eq!(s.journal_list_since(bank, 0, 10).expect("list").len(), 1);
    assert_eq!(s.journal_max_seq(bank).expect("max"), 1);
    assert!(
        s.journal_apply_guard_hit(bank, "item", "item-m1", "upsert", "hash-m1")
            .expect("guard")
    );

    s.cursor_set(bank, "push", "tok-1").expect("set");
    assert_eq!(
        s.cursor_get(bank, "push").expect("get"),
        Some("tok-1".into())
    );

    let conflict = SyncConflictRow {
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: "item-m1".into(),
        winner_mutation_id: "m1".into(),
        loser_mutation_id: "m0".into(),
        winner_transaction_time: "2026-06-02T00:00:00Z".into(),
        loser_transaction_time: "2026-06-01T00:00:00Z".into(),
        winner_device_id: "dev-a".into(),
        loser_device_id: "dev-b".into(),
        created_at: "2026-06-02T00:00:00Z".into(),
    };
    s.conflict_insert(&conflict).expect("insert");
    assert_eq!(s.conflict_count(bank).expect("count"), 1);
    assert_eq!(s.conflict_list(bank, 10).expect("list").len(), 1);

    let dlq = SyncDeadLetterRow {
        sync_mutation_id: "m-poison".into(),
        bank_id: bank.into(),
        reason_code: "malformed".into(),
        detail: "bad".into(),
        envelope_json: "{}".into(),
        status: DeadLetterStatus::Open,
        first_seen: "2026-06-01T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    };
    s.dead_letter_open(&dlq).expect("open");
    assert_eq!(
        s.dead_letter_status("m-poison").expect("status"),
        Some(DeadLetterStatus::Open)
    );
    assert_eq!(
        s.dead_letter_count(bank, DeadLetterStatus::Open)
            .expect("count"),
        1
    );
    assert!(
        s.dead_letter_ack_skip("m-poison", "2026-06-03T00:00:00Z")
            .expect("ack")
    );
    assert_eq!(
        s.dead_letter_list(bank, DeadLetterStatus::AckedSkip, 10)
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn high_water_and_erasure_probes_delegate() {
    let s = store();
    let bank = "bank-probe";
    assert!(
        s.device_high_water_get(bank, "dev-a", "item")
            .expect("get")
            .is_none()
    );
    s.device_high_water_set(bank, "dev-a", "item", "2026-06-01T00:00:00Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get(bank, "dev-a", "item").expect("get"),
        Some("2026-06-01T00:00:00Z".into())
    );

    let gone = "subject-gone";
    assert!(!s.subject_is_shredded(gone).expect("probe"));
    s.upsert_subject_key_ref(gone, "shredded", "shredded")
        .expect("shred");
    assert!(s.subject_is_shredded(gone).expect("probe"));
    assert!(!s.persona_context_erased(bank).expect("probe"));
}

#[test]
fn change_feed_arms_read_the_bank() {
    let s = store();
    let bank = "bank-feed";
    let mut item = carrier(bank, "item-1", EpisodicType::Task);
    item.kind = ItemKind::Semantic;
    item.episodic_type = None;
    item.category = Some(clio_types::SemanticCategory::ToolConfig);
    s.create_memory_item(&item, "seed").expect("create");

    assert_eq!(s.feed_items(bank, SENTINEL, 10).expect("feed").len(), 1);
    assert_eq!(s.feed_assoc(bank, SENTINEL, 10).expect("feed").len(), 0);
    assert_eq!(s.feed_triples(bank, SENTINEL, 10).expect("feed").len(), 0);
    assert_eq!(
        s.feed_persona_stable(bank, SENTINEL, 10)
            .expect("feed")
            .len(),
        0
    );
    assert_eq!(
        s.feed_persona_preferences(bank, SENTINEL, 10)
            .expect("feed")
            .len(),
        0
    );
    assert_eq!(s.feed_beliefs(bank, SENTINEL, 10).expect("feed").len(), 0);
    assert!(
        s.feed_tasks(bank, SENTINEL, 10).expect("feed").is_empty(),
        "no task row is committed yet"
    );
    assert!(
        s.feed_failures(bank, SENTINEL, 10)
            .expect("feed")
            .is_empty(),
        "no failure row is committed yet"
    );
}

#[test]
fn persona_apply_arms_roundtrip() {
    let s = store();
    let bank = "bank-persona";

    assert!(
        s.persona_stable_current(bank, "style")
            .expect("current")
            .is_none()
    );
    let entry = clio_types::PersonaStableEntry {
        bank_id: bank.into(),
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
    s.persona_stable_put(&entry, "seed").expect("put");
    assert_eq!(
        s.persona_stable_current(bank, "style")
            .expect("current")
            .expect("entry")
            .value,
        "terse"
    );

    assert!(
        s.persona_preference_current(bank, "brevity")
            .expect("current")
            .is_none()
    );
    let pref = clio_types::PersonaPreference {
        bank_id: bank.into(),
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
    s.persona_preference_put(&pref, "seed").expect("put");
    assert_eq!(
        s.persona_preference_current(bank, "brevity")
            .expect("current")
            .expect("pref")
            .value,
        0.4
    );
}

#[test]
fn belief_apply_arms_roundtrip() {
    let s = store();
    let bank = "bank-belief";

    let belief = clio_types::BeliefObject {
        id: "belief-1".into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        proposition: "user prefers oat milk".into(),
        context: Some("stated twice".into()),
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.7, "ev", SourceType::UserStated).expect("entry"),
        ],
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
    };
    assert!(s.belief_get(bank, "belief-1").expect("get").is_none());
    s.belief_create(&belief, "seed").expect("create");
    assert_eq!(
        s.belief_get(bank, "belief-1")
            .expect("get")
            .expect("belief")
            .proposition,
        "user prefers oat milk"
    );
    let key = clio_types::proposition_key(&belief.proposition);
    assert!(
        s.belief_by_proposition(bank, &key)
            .expect("by proposition")
            .is_some(),
        "the proposition key finds the belief"
    );
    let entry =
        ConfidenceEntry::new("2026-06-02", 0.8, "ev2", SourceType::UserStated).expect("entry");
    s.belief_append(bank, "belief-1", &entry, "seed", Some("stated twice"))
        .expect("append");
    assert_eq!(
        s.belief_get(bank, "belief-1")
            .expect("get")
            .expect("belief")
            .confidence_history
            .len(),
        2
    );
    s.belief_correct_context(bank, "belief-1", "corrected", "test", "agent")
        .expect("correct");
    assert_eq!(
        s.belief_get(bank, "belief-1")
            .expect("get")
            .expect("belief")
            .context
            .as_deref(),
        Some("corrected")
    );
}

#[test]
fn history_apply_arms_roundtrip() {
    let s = store();
    let bank = "bank-history";

    let item = carrier(bank, "task-item", EpisodicType::Task);
    let stored = s
        .task_put(&item, &task_draft(bank, "task-item"), "sync-apply")
        .expect("task put");
    assert_eq!(stored.version, 1);
    assert_eq!(
        s.task_current(bank, "task-sync")
            .expect("current")
            .expect("row")
            .id,
        stored.id
    );
    assert_eq!(s.feed_tasks(bank, SENTINEL, 10).expect("feed").len(), 1);

    let mut failure_item = carrier(bank, "failure-item", EpisodicType::Failure);
    failure_item.epistemic_kind = EpistemicKind::Belief;
    failure_item.source_type = Some(SourceType::AgentInferred);
    let failure = s
        .failure_put(
            &failure_item,
            &failure_draft(bank, "failure-item"),
            "sync-apply",
        )
        .expect("failure put");
    assert_eq!(failure.attempt_n, 2);
    assert_eq!(
        s.failure_by_attempt(bank, "task-sync", 2)
            .expect("by attempt")
            .expect("row")
            .id,
        failure.id
    );
    assert_eq!(s.feed_failures(bank, SENTINEL, 10).expect("feed").len(), 1);
}
