// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests every SQLite sync-state SQL body directly: the journal
//! reads and the idempotency guard, the opaque cursor token, the per-device
//! high-water mark, the LWW conflict journal, the dead-letter quarantine, and
//! the two erasure probes.
//!
//! ## Owns
//! - SQLite sync-state body coverage: journal, cursor, high-water, conflict,
//!   dead-letter, shredded-subject, and persona-erasure probes
//!
//! ## Does not own
//! - The `SyncStore for SqliteStore` delegation surface (`sqlite_sync_tests`)
//! - The journal append transaction and change feed (`sync_records`)
//! - Postgres sync-state bodies (`postgres_sync_state_tests`)
//!
//! ## Boundary
//!
//! Tests here break when SQLite sync-state SQL changes; they never change it.
//!
//! Keep this module focused on the SQLite sync-state bodies.

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

fn journal_row(id: &str, bank: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: id.into(),
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{id}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: format!("hash-{id}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn conflict(bank: &str, loser: &str, created_at: &str) -> SyncConflictRow {
    SyncConflictRow {
        bank_id: bank.into(),
        entity_kind: "item".into(),
        entity_id: "item-1".into(),
        winner_mutation_id: "mut-winner".into(),
        loser_mutation_id: loser.into(),
        winner_transaction_time: "2026-01-02T00:00:00Z".into(),
        loser_transaction_time: "2026-01-01T00:00:00Z".into(),
        winner_device_id: "dev-a".into(),
        loser_device_id: "dev-b".into(),
        created_at: created_at.into(),
    }
}

fn dead_letter(id: &str, bank: &str) -> SyncDeadLetterRow {
    SyncDeadLetterRow {
        sync_mutation_id: id.into(),
        bank_id: bank.into(),
        reason_code: "malformed".into(),
        detail: "bad envelope".into(),
        envelope_json: "{}".into(),
        status: DeadLetterStatus::Open,
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    }
}

#[test]
fn journal_reads_are_bank_scoped_and_seq_ordered() {
    let s = store();
    s.journal_append(&journal_row("m1", "b1")).expect("append");
    s.journal_append(&journal_row("m2", "b1")).expect("append");
    s.journal_append(&journal_row("m1", "b2")).expect("append");

    let got = s.journal_get("b1", "m1").expect("get").expect("row");
    assert_eq!(got.sync_mutation_id, "m1");
    assert_eq!(got.entity_kind, "item");
    assert_eq!(got.op, "upsert");
    assert!(
        s.journal_get("b1", "m-none").expect("get").is_none(),
        "an unknown mutation id has no row"
    );
    assert!(
        s.journal_get("b3", "m1").expect("get").is_none(),
        "the journal read is bank scoped"
    );

    let rows = s.journal_list_since("b1", 0, 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sync_mutation_id, "m1");
    assert_eq!(rows[1].sync_mutation_id, "m2");
    let paged = s.journal_list_since("b1", 1, 10).expect("list");
    assert_eq!(paged.len(), 1, "since_seq is exclusive");
    assert_eq!(s.journal_list_since("b1", 0, 1).expect("list").len(), 1);
    assert_eq!(s.journal_list_since("b9", 0, 10).expect("list").len(), 0);
}

#[test]
fn journal_max_seq_starts_at_zero_and_counts_one_bank() {
    let s = store();
    assert_eq!(s.journal_max_seq("b1").expect("max"), 0);
    s.journal_append(&journal_row("m1", "b1")).expect("append");
    s.journal_append(&journal_row("m2", "b1")).expect("append");
    assert_eq!(s.journal_max_seq("b1").expect("max"), 2);
    assert_eq!(
        s.journal_max_seq("b2").expect("max"),
        0,
        "another bank has no journal rows"
    );
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
            .expect("guard"),
        "a different op is not a hit"
    );
    assert!(
        !s.journal_apply_guard_hit("b1", "item", "item-g1", "reinforce", "hash-g1")
            .expect("guard"),
        "a different entity kind is not a hit"
    );
    assert!(
        !s.journal_apply_guard_hit("b1", "assoc_edge", "item-g1", "reinforce", "hash-zzz")
            .expect("guard"),
        "a different content hash is not a hit"
    );
    assert!(
        !s.journal_apply_guard_hit("b2", "assoc_edge", "item-g1", "reinforce", "hash-g1")
            .expect("guard"),
        "the guard is bank scoped"
    );
}

#[test]
fn cursor_roundtrip_upserts_per_bank_and_peer() {
    let s = store();
    assert!(s.cursor_get("b1", "push").expect("get").is_none());
    s.cursor_set("b1", "push", "tok-1").expect("set");
    s.cursor_set("b1", "pull", "tok-2").expect("set");
    s.cursor_set("b1", "push", "tok-3").expect("set");
    assert_eq!(
        s.cursor_get("b1", "push").expect("get"),
        Some("tok-3".into()),
        "a second set replaces the token"
    );
    assert_eq!(
        s.cursor_get("b1", "pull").expect("get"),
        Some("tok-2".into())
    );
    assert!(
        s.cursor_get("b2", "push").expect("get").is_none(),
        "cursors are bank scoped"
    );
}

#[test]
fn high_water_starts_empty_and_never_moves_backwards() {
    let s = store();
    assert!(
        s.device_high_water_get("b1", "dev-a", "item")
            .expect("get")
            .is_none()
    );
    s.device_high_water_set("b1", "dev-a", "item", "2026-01-01T00:00:02Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get("b1", "dev-a", "item").expect("get"),
        Some("2026-01-01T00:00:02Z".into())
    );
    s.device_high_water_set("b1", "dev-a", "item", "2026-01-01T00:00:01Z")
        .expect("set a lower stamp");
    assert_eq!(
        s.device_high_water_get("b1", "dev-a", "item").expect("get"),
        Some("2026-01-01T00:00:02Z".into()),
        "a lower stamp never lowers the mark"
    );
    s.device_high_water_set("b1", "dev-a", "item", "2026-01-01T00:00:03Z")
        .expect("set a higher stamp");
    assert_eq!(
        s.device_high_water_get("b1", "dev-a", "item").expect("get"),
        Some("2026-01-01T00:00:03Z".into()),
        "a higher stamp raises the mark"
    );
    assert!(
        s.device_high_water_get("b1", "dev-b", "item")
            .expect("get")
            .is_none(),
        "each device keeps its own mark"
    );
    assert!(
        s.device_high_water_get("b2", "dev-a", "item")
            .expect("get")
            .is_none(),
        "each bank keeps its own mark"
    );
}

#[test]
fn high_water_is_independent_per_entity_kind() {
    let s = store();
    s.device_high_water_set("b1", "dev-a", "item", "2026-01-02T00:00:00Z")
        .expect("set the item mark");
    s.device_high_water_set("b1", "dev-a", "triple", "2026-01-01")
        .expect("set the triple mark");
    assert_eq!(
        s.device_high_water_get("b1", "dev-a", "item").expect("get"),
        Some("2026-01-02T00:00:00Z".into()),
        "the item mark keeps its own stamp"
    );
    assert_eq!(
        s.device_high_water_get("b1", "dev-a", "triple")
            .expect("get"),
        Some("2026-01-01".into()),
        "the triple mark is not lowered by the later item stamp"
    );
    assert!(
        s.device_high_water_get("b1", "dev-a", "belief")
            .expect("get")
            .is_none(),
        "an unwritten kind has no mark"
    );
}

#[test]
fn conflict_rows_are_idempotent_and_newest_first() {
    let s = store();
    assert_eq!(s.conflict_count("b1").expect("count"), 0);
    s.conflict_insert(&conflict("b1", "mut-l1", "2026-01-01T00:00:00Z"))
        .expect("insert");
    s.conflict_insert(&conflict("b1", "mut-l1", "2026-01-01T00:00:00Z"))
        .expect("replay");
    assert_eq!(
        s.conflict_count("b1").expect("count"),
        1,
        "the loser id keys the row, so a replay is ignored"
    );
    s.conflict_insert(&conflict("b1", "mut-l2", "2026-01-02T00:00:00Z"))
        .expect("insert");
    s.conflict_insert(&conflict("b2", "mut-l3", "2026-01-03T00:00:00Z"))
        .expect("insert other bank");

    assert_eq!(s.conflict_count("b1").expect("count"), 2);
    let rows = s.conflict_list("b1", 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].loser_mutation_id, "mut-l2",
        "newest created_at first"
    );
    assert_eq!(rows[1].loser_mutation_id, "mut-l1");
    assert_eq!(rows[0].winner_device_id, "dev-a");
    assert_eq!(s.conflict_list("b1", 1).expect("list").len(), 1);
    assert_eq!(s.conflict_count("b2").expect("count"), 1);
}

#[test]
fn dead_letter_open_reopen_ack_and_list() {
    let s = store();
    assert_eq!(
        s.dead_letter_status("m-none").expect("status"),
        None,
        "an unknown mutation has no status"
    );

    let row = dead_letter("m-bad", "b1");
    s.dead_letter_open(&row).expect("open");
    assert_eq!(
        s.dead_letter_status("m-bad").expect("status"),
        Some(DeadLetterStatus::Open)
    );
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::Open)
            .expect("count"),
        1
    );
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::AckedSkip)
            .expect("count"),
        0
    );
    let listed = s
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].reason_code, "malformed");
    assert_eq!(
        listed[0].first_seen, "2026-01-01T00:00:00Z",
        "the first sighting is kept"
    );

    // Reopening updates the reason and the detail but keeps the first sighting.
    let mut reopened = dead_letter("m-bad", "b1");
    reopened.reason_code = "untrusted".into();
    reopened.detail = "signature".into();
    reopened.first_seen = "2026-02-02T00:00:00Z".into();
    reopened.last_error = Some("boom".into());
    s.dead_letter_open(&reopened).expect("reopen");
    let listed = s
        .dead_letter_list("b1", DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(listed[0].reason_code, "untrusted");
    assert_eq!(listed[0].last_error.as_deref(), Some("boom"));
    assert_eq!(
        listed[0].first_seen, "2026-01-01T00:00:00Z",
        "a reopen does not move the first sighting"
    );

    assert!(
        s.dead_letter_ack_skip("m-bad", "2026-03-03T00:00:00Z")
            .expect("ack")
    );
    assert!(
        !s.dead_letter_ack_skip("m-bad", "2026-03-04T00:00:00Z")
            .expect("ack"),
        "a second ack changes nothing"
    );
    assert_eq!(
        s.dead_letter_status("m-bad").expect("status"),
        Some(DeadLetterStatus::AckedSkip)
    );
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::AckedSkip)
            .expect("count"),
        1
    );
    assert_eq!(
        s.dead_letter_count("b1", DeadLetterStatus::Open)
            .expect("count"),
        0
    );
    assert_eq!(
        s.dead_letter_list("b1", DeadLetterStatus::AckedSkip, 10)
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        s.dead_letter_list("b9", DeadLetterStatus::Open, 10)
            .expect("list")
            .len(),
        0
    );
}

#[test]
fn subject_is_shredded_reads_the_key_status() {
    let s = store();
    let live = "subject-live";
    let gone = "subject-gone";
    assert!(!s.subject_is_shredded(live).expect("probe"));
    assert!(
        !s.subject_is_shredded("subject-absent").expect("probe"),
        "an absent subject key is not shredded"
    );
    s.upsert_subject_key_ref(gone, "shredded", "shredded")
        .expect("shred");
    assert!(s.subject_is_shredded(gone).expect("probe"));
}

#[test]
fn persona_context_erased_reads_the_tombstone() {
    let s = store();
    assert!(!s.persona_context_erased("b1").expect("probe"));
    s.test_execute(
        "INSERT INTO persona_erase_tombstones (bank_id, erased_at) \
         VALUES ('b1', '2026-01-01T00:00:00Z')",
    )
    .expect("tombstone");
    assert!(s.persona_context_erased("b1").expect("probe"));
    assert!(
        !s.persona_context_erased("b2").expect("probe"),
        "the probe is bank scoped"
    );
}
