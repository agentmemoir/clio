// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite `SyncStore` contract on an in-memory store:
//! journal append idempotence and ordering, cursor storage, conflict rows,
//! dead-letter quarantine with ack-skip, and the apply guard.
//!
//! ## Owns
//! - SQLite `SyncStore` journal / cursor / conflict / dead-letter assertions
//!
//! ## Does not own
//! - Change-feed assertions (`sync_store_feed_tests`)
//! - Postgres sync parity (`pg_sync_tests`)
//! - Sync protocol semantics (`clio-sync`)
//!
//! ## Boundary
//!
//! Tests here break when SQLite sync SQL changes; they never change it.
//!
//! Keep this module focused on SQLite sync persistence tests.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn journal_row(seq_hint: &str, bank: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: format!("mut-{seq_hint}"),
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{seq_hint}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:0Z".into(),
        device_id: "dev-a".into(),
        content_hash: format!("hash-{seq_hint}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: Some(r#"{"admitted":true}"#.into()),
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn journal_append_is_idempotent_and_commit_ordered() {
    let s = store();
    let out1 = s.journal_append(&journal_row("a", "b1")).expect("append");
    assert!(out1.inserted);
    assert_eq!(out1.seq, 1);
    let replay = s.journal_append(&journal_row("a", "b1")).expect("replay");
    assert!(!replay.inserted);
    assert_eq!(replay.seq, 1);
    let out2 = s.journal_append(&journal_row("b", "b1")).expect("append");
    assert_eq!(out2.seq, 2);
    // Same mutation id in a different bank is a separate journal entry.
    let other = s.journal_append(&journal_row("a", "b2")).expect("append");
    assert_eq!(other.seq, 1);
    assert_eq!(s.journal_max_seq("b1").expect("max"), 2);
    assert_eq!(s.journal_max_seq("missing").expect("max"), 0);

    let rows = s.journal_list_since("b1", 0, 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sync_mutation_id, "mut-a");
    assert_eq!(rows[1].seq, 2);
    let paged = s.journal_list_since("b1", 1, 10).expect("list");
    assert_eq!(paged.len(), 1);
    assert_eq!(s.journal_list_since("b1", 0, 1).expect("list").len(), 1);

    let got = s.journal_get("b1", "mut-a").expect("get").expect("row");
    assert_eq!(got.entity_kind, "item");
    assert_eq!(got.op, "upsert");
    assert!(s.journal_get("b1", "mut-zzz").expect("get").is_none());
    assert!(s.journal_get("b2", "mut-b").expect("get").is_none());
}

#[test]
fn journal_apply_guard_matches_identity_and_hash() {
    let s = store();
    let mut row = journal_row("g1", "b1");
    row.entity_kind = "assoc_edge".into();
    row.op = "reinforce".into();
    s.journal_append(&row).expect("append");
    assert!(
        s.journal_apply_guard_hit("b1", "assoc_edge", "item-g1", "reinforce", "hash-g1")
            .expect("guard")
    );
    assert!(
        !s.journal_apply_guard_hit("b1", "assoc_edge", "item-g1", "upsert", "hash-g1")
            .expect("guard")
    );
    assert!(
        !s.journal_apply_guard_hit("b1", "item", "item-g1", "reinforce", "hash-g1")
            .expect("guard")
    );
}

#[test]
fn cursors_roundtrip_per_bank_and_peer() {
    let s = store();
    assert!(s.cursor_get("b1", "push").expect("get").is_none());
    s.cursor_set("b1", "push", "tok-1").expect("set");
    s.cursor_set("b1", "pull", "tok-2").expect("set");
    s.cursor_set("b2", "push", "tok-3").expect("set");
    s.cursor_set("b1", "push", "tok-4").expect("set");
    assert_eq!(
        s.cursor_get("b1", "push").expect("get"),
        Some("tok-4".into())
    );
    assert_eq!(
        s.cursor_get("b1", "pull").expect("get"),
        Some("tok-2".into())
    );
    assert_eq!(
        s.cursor_get("b2", "push").expect("get"),
        Some("tok-3".into())
    );
}

#[test]
fn conflicts_record_once_and_count() {
    let s = store();
    let row = SyncConflictRow {
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: "item-x".into(),
        winner_mutation_id: "mut-w".into(),
        loser_mutation_id: "mut-l".into(),
        winner_transaction_time: "2026-01-02T00:00:00Z".into(),
        loser_transaction_time: "2026-01-01T00:00:00Z".into(),
        winner_device_id: "dev-a".into(),
        loser_device_id: "dev-b".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
    };
    s.conflict_insert(&row).expect("insert");
    s.conflict_insert(&row).expect("insert idempotent");
    assert_eq!(s.conflict_count("b1").expect("count"), 1);
    let rows = s.conflict_list("b1", 10).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].winner_mutation_id, "mut-w");
    assert_eq!(s.conflict_count("b2").expect("count"), 0);
}

#[test]
fn dead_letters_open_reopen_and_ack_skip() {
    let s = store();
    let mut row = dlq_row("mut-poison");
    s.dead_letter_open(&row).expect("open");
    row.detail = "erased subject resurrection attempt".into();
    s.dead_letter_open(&row).expect("reopen keeps first_seen");
    let rows = s
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_seen, "2026-01-01T00:00:00Z");
    assert_eq!(rows[0].detail, "erased subject resurrection attempt");
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::Open)
            .expect("count"),
        1
    );

    // Unknown id does not ack.
    assert!(
        !s.dead_letter_ack_skip("mut-unknown", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    assert!(
        s.dead_letter_ack_skip("mut-poison", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    // Ack is one-shot: second ack on an already-acked row reports false.
    assert!(
        !s.dead_letter_ack_skip("mut-poison", "2026-01-04T00:00:00Z")
            .expect("ack")
    );
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::AckedSkip)
            .expect("count"),
        1
    );
    let open_dead_letters = s
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert!(open_dead_letters.is_empty(), "{open_dead_letters:?}");
}

fn dlq_row(id: &str) -> SyncDeadLetterRow {
    SyncDeadLetterRow {
        sync_mutation_id: id.into(),
        bank_id: "b1".into(),
        reason_code: "erased_subject".into(),
        detail: "erased subject resurrect".into(),
        envelope_json: r#"{"sync_mutation_id":"mut-poison"}"#.into(),
        status: DeadLetterStatus::Open,
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    }
}

#[test]
fn dead_letter_status_and_shredded_probe_read_back() {
    let s = store();
    // No dead-letter row yet for an unknown id.
    assert_eq!(s.dead_letter_status("mut-none").expect("status"), None);
    s.dead_letter_open(&dlq_row("mut-poison")).expect("open");
    assert_eq!(
        s.dead_letter_status("mut-poison").expect("status"),
        Some(DeadLetterStatus::Open)
    );
    assert!(
        s.dead_letter_ack_skip("mut-poison", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    assert_eq!(
        s.dead_letter_status("mut-poison").expect("status"),
        Some(DeadLetterStatus::AckedSkip)
    );
    // Shredded-subject fail-closed probe.
    assert!(!s.subject_is_shredded("subj-live").expect("probe"));
    s.upsert_subject_key_ref("subj-gone", "shredded", "shredded")
        .expect("shred marker");
    assert!(s.subject_is_shredded("subj-gone").expect("probe"));
}

#[path = "sync_store_tests_feed_tests.rs"]
mod feed_tests;
