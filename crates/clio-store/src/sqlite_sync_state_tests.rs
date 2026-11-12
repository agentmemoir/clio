// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite sync-state SQL bodies: the opaque cursor
//! upsert, the LWW conflict journal, the dead-letter quarantine, the erasure
//! probes, and the row limits (an oversized `usize` must reach SQLite as a
//! bounded positive `LIMIT`, never as the negative value SQLite reads as "no
//! limit").
//!
//! ## Owns
//! - Coverage for `limit_i64`, the journal row limit, and the cursor /
//!   conflict / dead-letter / erasure-probe SQL bodies in `sqlite_sync_state`
//!
//! ## Does not own
//! - The journal append transaction or conflict policy (`sqlite_sync`,
//!   `sync_records`)
//! - Postgres sync state SQL (`postgres_sync_state`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::limit_i64;
use crate::sqlite::sqlite_history_tests::store;
use crate::store::Store;
use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};

fn row(mid: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: mid.into(),
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: format!("i-{mid}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: format!("h-{mid}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn journal_list_since_bounds_an_oversized_limit_to_the_available_rows() {
    // A wrapped negative `LIMIT` also returns every available row, so the
    // saturated value is what pins the conversion itself.
    assert_eq!(limit_i64(usize::MAX), i64::MAX);
    let store = store();
    for n in 0..3 {
        store
            .journal_append(&row(&format!("m-{n}")))
            .expect("append");
    }
    let rows = store
        .journal_list_since("b1", 0, usize::MAX)
        .expect("journal list");
    let ids = rows
        .iter()
        .map(|r| r.sync_mutation_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["m-0", "m-1", "m-2"]);
}

#[test]
fn journal_apply_guard_reads_a_journaled_mutation() {
    let store = store();
    store.journal_append(&row("guard")).expect("append");
    assert!(
        store
            .journal_apply_guard_hit("b1", "item", "i-guard", "upsert", "h-guard")
            .expect("guard")
    );
    assert!(
        !store
            .journal_apply_guard_hit("b1", "item", "i-guard", "upsert", "h-other")
            .expect("guard")
    );
    assert_eq!(
        store
            .journal_content_hashes("b1", "item", "i-guard", "upsert")
            .expect("hashes"),
        ["h-guard"]
    );
    assert!(
        store
            .journal_content_hashes("b1", "item", "i-guard", "invalidate")
            .expect("hashes")
            .is_empty()
    );
}

#[test]
fn cursor_set_upserts_per_bank_and_peer() {
    let store = store();
    assert!(store.cursor_get("b1", "push").expect("get").is_none());
    store.cursor_set("b1", "push", "tok-1").expect("set");
    store.cursor_set("b1", "pull", "tok-2").expect("set");
    store.cursor_set("b2", "push", "tok-3").expect("set");
    store.cursor_set("b1", "push", "tok-4").expect("set");
    assert_eq!(
        store.cursor_get("b1", "push").expect("get"),
        Some("tok-4".into())
    );
    assert_eq!(
        store.cursor_get("b1", "pull").expect("get"),
        Some("tok-2".into())
    );
    assert_eq!(
        store.cursor_get("b2", "push").expect("get"),
        Some("tok-3".into())
    );
    assert!(store.cursor_get("b3", "push").expect("get").is_none());
}

fn conflict(loser: &str, created_at: &str) -> SyncConflictRow {
    SyncConflictRow {
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: "item-x".into(),
        winner_mutation_id: format!("mut-w-{loser}"),
        loser_mutation_id: format!("mut-{loser}"),
        winner_transaction_time: "2026-01-02T00:00:00Z".into(),
        loser_transaction_time: "2026-01-01T00:00:00Z".into(),
        winner_device_id: "dev-a".into(),
        loser_device_id: "dev-b".into(),
        created_at: created_at.into(),
    }
}

#[test]
fn conflict_insert_is_idempotent_and_list_is_newest_first() {
    let store = store();
    let older = conflict("l1", "2026-01-01T00:00:00Z");
    store.conflict_insert(&older).expect("insert");
    store.conflict_insert(&older).expect("replay insert");
    store
        .conflict_insert(&conflict("l2", "2026-01-02T00:00:00Z"))
        .expect("insert");
    assert_eq!(store.conflict_count("b1").expect("count"), 2);

    let rows = store.conflict_list("b1", 10).expect("list");
    let losers = rows
        .iter()
        .map(|r| r.loser_mutation_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(losers, ["mut-l2", "mut-l1"]);
    assert_eq!(rows[0].winner_mutation_id, "mut-w-l2");
    assert_eq!(rows[0].winner_device_id, "dev-a");
    assert_eq!(rows[1].loser_device_id, "dev-b");
    assert_eq!(rows[1].created_at, "2026-01-01T00:00:00Z");
    assert_eq!(store.conflict_list("b1", 1).expect("list").len(), 1);
    let other_bank = store.conflict_list("b2", 10).expect("list");
    assert!(other_bank.is_empty(), "{other_bank:?}");
}

fn dlq(id: &str, first_seen: &str) -> SyncDeadLetterRow {
    SyncDeadLetterRow {
        sync_mutation_id: id.into(),
        bank_id: "b1".into(),
        reason_code: "erased_subject".into(),
        detail: "erased subject resurrect".into(),
        envelope_json: format!(r#"{{"sync_mutation_id":"{id}"}}"#),
        status: DeadLetterStatus::Open,
        first_seen: first_seen.into(),
        last_error: None,
        updated_at: None,
    }
}

#[test]
fn dead_letter_list_and_ack_skip_move_rows_between_statuses() {
    let store = store();
    store
        .dead_letter_open(&dlq("mut-a", "2026-01-01T00:00:00Z"))
        .expect("open");
    store
        .dead_letter_open(&dlq("mut-b", "2026-01-02T00:00:00Z"))
        .expect("open");
    let open = store
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    let ids = open
        .iter()
        .map(|r| r.sync_mutation_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["mut-a", "mut-b"]);
    assert_eq!(open[0].reason_code, "erased_subject");
    assert_eq!(
        open[0].envelope_json,
        r#"{"sync_mutation_id":"mut-a"}"#.to_owned()
    );
    assert_eq!(
        store
            .dead_letter_list("b1", DeadLetterStatus::Open, 1)
            .expect("list")
            .len(),
        1
    );
    assert!(
        store
            .dead_letter_list("b2", DeadLetterStatus::Open, 10)
            .expect("list")
            .is_empty(),
        "bank isolation"
    );

    assert!(
        !store
            .dead_letter_ack_skip("mut-unknown", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    assert!(
        store
            .dead_letter_ack_skip("mut-a", "2026-01-03T00:00:00Z")
            .expect("ack")
    );
    assert!(
        !store
            .dead_letter_ack_skip("mut-a", "2026-01-04T00:00:00Z")
            .expect("ack is one-shot")
    );
    assert_eq!(
        store.dead_letter_status("mut-a").expect("status"),
        Some(DeadLetterStatus::AckedSkip)
    );
    let acked = store
        .dead_letter_list("b1", DeadLetterStatus::AckedSkip, 10)
        .expect("list");
    assert_eq!(acked.len(), 1);
    assert_eq!(acked[0].sync_mutation_id, "mut-a");
    assert_eq!(acked[0].updated_at.as_deref(), Some("2026-01-03T00:00:00Z"));
    let still_open = store
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(still_open.len(), 1);
    assert_eq!(still_open[0].sync_mutation_id, "mut-b");
}

#[test]
fn dead_letter_reopen_keeps_the_first_seen_stamp() {
    let store = store();
    let mut reopened = dlq("mut-poison", "2026-01-01T00:00:00Z");
    store.dead_letter_open(&reopened).expect("open");
    reopened.first_seen = "2026-01-05T00:00:00Z".into();
    reopened.detail = "second failure".into();
    store.dead_letter_open(&reopened).expect("reopen");
    let rows = store
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_seen, "2026-01-01T00:00:00Z");
    assert_eq!(rows[0].detail, "second failure");
}

#[test]
fn subject_is_shredded_only_for_a_shredded_subject_key() {
    let store = store();
    assert!(!store.subject_is_shredded("subj-absent").expect("probe"));
    store
        .upsert_subject_key_ref("subj-live", "dek-live", "active")
        .expect("key");
    assert!(!store.subject_is_shredded("subj-live").expect("probe"));
    store
        .upsert_subject_key_ref("subj-gone", "dek-gone", "shredded")
        .expect("key");
    assert!(store.subject_is_shredded("subj-gone").expect("probe"));
}

#[test]
fn persona_context_erased_follows_the_bank_tombstone() {
    let store = store();
    assert!(!store.persona_context_erased("b1").expect("probe"));
    store
        .test_execute(
            "INSERT INTO persona_erase_tombstones (bank_id, erased_at) \
             VALUES ('b1', '2026-01-01T00:00:00Z')",
        )
        .expect("tombstone");
    assert!(store.persona_context_erased("b1").expect("probe"));
    assert!(!store.persona_context_erased("b2").expect("probe"));
}
