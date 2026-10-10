// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests Postgres parity for the `SyncStore` contract on the
//! shared Compose `clio` database: journal append idempotence, cursor
//! storage, conflicts, dead-letter ack-skip, and the change-feed queries.
//!
//! ## Owns
//! - Postgres parity coverage for the `SyncStore` contract
//!
//! ## Does not own
//! - SQLite-path sync behavior (`sync_store_tests`)
//! - Sync protocol semantics (`clio-sync`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres sync SQL changes; they never change it.
//!
//! Keep this module focused on Postgres sync parity tests.

use crate::db_url::postgres_url_from_env;
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::pg_lock::lock_pg_test;
use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};
use crate::{DekProvider, LocalDevKms, PostgresStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem, TripleRecord};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

use crate::test_unique::unique;

fn journal_row(id: &str, bank: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: id.into(),
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{id}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-pg".into(),
        content_hash: format!("hash-{id}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn pg_sync_journal_cursors_conflict_dlq_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();

    let out1 = s.journal_append(&journal_row("m1", &bank)).expect("append");
    assert!(out1.inserted);
    assert_eq!(out1.seq, 1);
    let replay = s.journal_append(&journal_row("m1", &bank)).expect("replay");
    assert!(!replay.inserted);
    assert_eq!(replay.seq, 1);
    let out2 = s.journal_append(&journal_row("m2", &bank)).expect("append");
    assert_eq!(out2.seq, 2);
    assert_eq!(s.journal_max_seq(&bank).expect("max"), 2);

    let rows = s.journal_list_since(&bank, 0, 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.bank_id == bank));
    let got = s.journal_get(&bank, "m1").expect("get").expect("row");
    assert_eq!(got.device_id, "dev-pg");
    assert!(s.journal_get(&bank, "m-none").expect("get").is_none());

    assert!(
        s.journal_apply_guard_hit(&bank, "item", "item-m1", "upsert", "hash-m1")
            .expect("guard")
    );
    assert!(
        !s.journal_apply_guard_hit(&bank, "item", "item-m1", "invalidate", "hash-m1")
            .expect("guard")
    );

    s.cursor_set(&bank, "push", "tok-1").expect("set");
    s.cursor_set(&bank, "push", "tok-2").expect("set");
    assert_eq!(
        s.cursor_get(&bank, "push").expect("get"),
        Some("tok-2".into())
    );

    let conflict = SyncConflictRow {
        bank_id: bank.clone(),
        entity_kind: "item".into(),
        entity_id: "item-x".into(),
        winner_mutation_id: "m2".into(),
        loser_mutation_id: "m1".into(),
        winner_transaction_time: "2026-01-02T00:00:00Z".into(),
        loser_transaction_time: "2026-01-01T00:00:00Z".into(),
        winner_device_id: "dev-pg".into(),
        loser_device_id: "dev-other".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
    };
    s.conflict_insert(&conflict).expect("insert");
    s.conflict_insert(&conflict).expect("idempotent");
    assert_eq!(s.conflict_count(&bank).expect("count"), 1);
    assert_eq!(s.conflict_list(&bank, 10).expect("list").len(), 1);

    let dlq = SyncDeadLetterRow {
        sync_mutation_id: "m-poison".into(),
        bank_id: bank.clone(),
        reason_code: "malformed".into(),
        detail: "bad envelope".into(),
        envelope_json: "{}".into(),
        status: DeadLetterStatus::Open,
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    };
    s.dead_letter_open(&dlq).expect("open");
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::Open)
            .expect("count"),
        1
    );
    assert!(
        s.dead_letter_ack_skip("m-poison", "2026-01-02T00:00:00Z")
            .expect("ack")
    );
    assert!(
        !s.dead_letter_ack_skip("m-poison", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::Open)
            .expect("count"),
        0
    );
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::AckedSkip)
            .expect("count"),
        1
    );
    assert_eq!(
        s.dead_letter_list(&bank, DeadLetterStatus::AckedSkip, 10)
            .expect("list")
            .len(),
        1
    );
}

fn carrier(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: None,
        snapshot: Some(serde_json::json!({"object": "Boston"})),
        gist: Some("lives_in Boston".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

#[test]
fn pg_sync_dead_letter_status_and_shredded_probe() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    assert_eq!(s.dead_letter_status("m-none").expect("status"), None);
    let dlq = SyncDeadLetterRow {
        sync_mutation_id: unique("m-dlq"),
        bank_id: bank,
        reason_code: "malformed".into(),
        detail: "bad".into(),
        envelope_json: "{}".into(),
        status: DeadLetterStatus::Open,
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    };
    s.dead_letter_open(&dlq).expect("open");
    assert_eq!(
        s.dead_letter_status(&dlq.sync_mutation_id).expect("status"),
        Some(DeadLetterStatus::Open)
    );
    s.dead_letter_ack_skip(&dlq.sync_mutation_id, "2026-01-03T00:00:00Z")
        .expect("ack");
    assert_eq!(
        s.dead_letter_status(&dlq.sync_mutation_id).expect("status"),
        Some(DeadLetterStatus::AckedSkip)
    );
    let live = unique("subj-live");
    let gone = unique("subj-gone");
    assert!(!s.subject_is_shredded(&live).expect("probe"));
    s.upsert_subject_key_ref(&gone, "shredded", "shredded")
        .expect("shred");
    assert!(s.subject_is_shredded(&gone).expect("probe"));
}

#[test]
fn pg_sync_feed_assoc_triples_and_seq_overflow() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    // A cursor sequence beyond `i32` is reported, never silently truncated.
    assert!(
        s.journal_list_since(&bank, i64::from(i32::MAX) + 1, 10)
            .is_err()
    );
    // The assoc feed maps a non-empty edge row.
    let a = unique("item-a");
    let b = unique("item-b");
    s.create_memory_item(&carrier(&a, &bank), "actor")
        .expect("a");
    s.create_memory_item(&carrier(&b, &bank), "actor")
        .expect("b");
    let edge = AssocEdgeRecord {
        id: unique("e"),
        bank_id: bank.clone(),
        source_id: a,
        target_id: b,
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.3,
        last_updated_at: "2026-01-02T00:00:00Z".into(),
    };
    s.put_assoc_edge(&edge).expect("edge");
    let feed = s
        .feed_assoc(&bank, "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].entity_kind, "assoc_edge");
    // The triple feed maps a non-empty row once closed on either axis.
    let car = carrier(&unique("car"), &bank);
    let subject = unique("s");
    let triple = TripleRecord {
        context: None,
        id: unique("tri"),
        bank_id: bank.clone(),
        item_id: car.id.clone(),
        subject: subject.clone(),
        predicate: "p".into(),
        object: "o".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
        snapshot_ref: None,
        gist_ref: None,
    };
    s.commit_triple_add(&car, &triple, false, "actor")
        .expect("add");
    s.triple_end(
        &bank,
        &subject,
        "p",
        Some("o"),
        "2026-01-02T00:00:00Z",
        "2026-01-02T00:00:00Z",
    )
    .expect("end");
    let feed = s
        .feed_triples(&bank, "2026-01-01T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].op, "invalidate");
}

#[test]
fn pg_sync_feed_reflects_item_changes() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();

    let item = clio_types::MemoryItem {
        id: format!("item-{}", unique("i")),
        bank_id: bank.clone(),
        subject_id: "subj".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    s.create_memory_item(&item, "actor").expect("create");
    let feed = s
        .feed_items(&bank, "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].op, "upsert");
    let assoc_feed = s
        .feed_assoc(&bank, "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert!(assoc_feed.is_empty(), "{assoc_feed:?}");
    let triple_feed = s
        .feed_triples(&bank, "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert!(triple_feed.is_empty(), "{triple_feed:?}");
}

/// Empty-watermark sentinel the client push cursor uses before any push.
const SENTINEL: &str = "1970-01-01T00:00:00Z";

/// The persona/belief record feeds and apply primitives return the same
/// durable rows on Postgres (server-blind ciphertext, bank-scoped).
#[test]
fn pg_sync_record_feeds_and_primitives_roundtrip() {
    use crate::{BeliefStore, PersonaStore};

    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();

    // Seed the domain records via the persona/belief store primitives.
    let entry = clio_types::PersonaStableEntry {
        bank_id: bank.clone(),
        key: "style".into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: Some("why recorded".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    };
    s.put_stable_entry(&entry, "seed").expect("put stable");
    let pref = clio_types::PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("preference context".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    s.put_preference_state(&pref, "seed").expect("pref put");
    let belief_id = format!("bel-{}", unique("pg"));
    let belief = clio_types::BeliefObject {
        id: belief_id.clone(),
        bank_id: bank.clone(),
        subject_id: "s1".into(),
        proposition: "user prefers oat milk".into(),
        context: Some("stated twice".into()),
        context_as_of: None,
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2026-01-01",
                0.7,
                "ev",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    s.create_belief(&belief, "seed").expect("belief create");

    // Versioned feeds surface each record with its change stamp.
    let stable_rows = s
        .feed_persona_stable(&bank, SENTINEL, 200)
        .expect("feed stable");
    assert_eq!(stable_rows.len(), 1);
    assert_eq!(stable_rows[0].1.key, "style");
    assert_eq!(stable_rows[0].1.value, "terse");
    let pref_rows = s
        .feed_persona_preferences(&bank, SENTINEL, 200)
        .expect("feed prefs");
    assert_eq!(pref_rows.len(), 1);
    assert_eq!(pref_rows[0].1.key, "brevity");
    let belief_rows = BeliefStore::list_beliefs(&s, &bank).expect("list beliefs");
    assert_eq!(belief_rows.len(), 1);
    assert_eq!(belief_rows[0].proposition, "user prefers oat milk");

    // Apply primitives read the same durable rows.
    let cur = s
        .persona_stable_current(&bank, "style")
        .expect("current")
        .expect("some");
    assert_eq!(cur.value, "terse");
    let bget = s
        .belief_get(&bank, &belief_id)
        .expect("belief get")
        .expect("some");
    assert_eq!(bget.proposition, "user prefers oat milk");

    // Watermarks filter unchanged rows out.
    let after = s
        .feed_beliefs(&bank, "9999-01-01T00:00:00Z", 200)
        .expect("feed after watermark");
    assert!(after.is_empty(), "watermarked rows must not re-feed");
}
