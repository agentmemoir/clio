// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests every Postgres sync-state SQL body directly: the journal
//! reads and the idempotency guard, the opaque cursor token, the per-device
//! high-water mark, the LWW conflict journal, the dead-letter quarantine, and
//! the two erasure probes.
//!
//! ## Owns
//! - Postgres sync-state body coverage: journal, cursor, high-water, conflict,
//!   dead-letter, shredded-subject, and persona-erasure probes
//!
//! ## Does not own
//! - The `SyncStore for PostgresStore` delegation surface (`postgres_sync_tests`)
//! - The journal append transaction and change feed (`postgres_sync_records`)
//! - SQLite sync-state bodies (`sqlite_sync_state_tests`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres sync-state SQL changes; they never change it.
//!
//! Keep this module focused on the Postgres sync-state bodies.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore, Store};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
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
        device_id: "dev-pg".into(),
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
        winner_device_id: "dev-pg".into(),
        loser_device_id: "dev-pg-other".into(),
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
fn pg_state_journal_reads_are_bank_scoped_and_ordered() {
    let _guard = lock_pg_test();
    let bank = unique("state-journal");
    let other = unique("state-journal-other");
    let s = open_pg();

    s.journal_append(&journal_row("m1", &bank)).expect("append");
    s.journal_append(&journal_row("m2", &bank)).expect("append");
    s.journal_append(&journal_row("m1", &other))
        .expect("append");

    let got = s.journal_get(&bank, "m1").expect("get").expect("row");
    assert_eq!(got.op, "upsert");
    assert!(s.journal_get(&bank, "m-none").expect("get").is_none());
    assert!(
        s.journal_get(&other, "m2").expect("get").is_none(),
        "the journal read is bank scoped"
    );

    let rows = s.journal_list_since(&bank, 0, 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sync_mutation_id, "m1");
    assert_eq!(rows[1].sync_mutation_id, "m2");
    assert_eq!(s.journal_list_since(&bank, 1, 10).expect("list").len(), 1);
    assert_eq!(s.journal_list_since(&bank, 0, 1).expect("list").len(), 1);
    assert_eq!(s.journal_max_seq(&bank).expect("max"), 2);
    assert_eq!(s.journal_max_seq(&unique("state-empty")).expect("max"), 0);

    let mut guarded = journal_row("g1", &bank);
    guarded.entity_kind = "assoc_edge".into();
    guarded.op = "reinforce".into();
    s.journal_append(&guarded).expect("append guarded");
    assert!(
        s.journal_apply_guard_hit(&bank, "assoc_edge", "item-g1", "reinforce", "hash-g1")
            .expect("guard")
    );
    assert!(
        !s.journal_apply_guard_hit(&bank, "assoc_edge", "item-g1", "upsert", "hash-g1")
            .expect("guard"),
        "a different op is not a hit"
    );
    assert!(
        !s.journal_apply_guard_hit(&bank, "item", "item-g1", "reinforce", "hash-g1")
            .expect("guard"),
        "a different entity kind is not a hit"
    );
    assert!(
        !s.journal_apply_guard_hit(&bank, "assoc_edge", "item-g1", "reinforce", "hash-zzz")
            .expect("guard"),
        "a different content hash is not a hit"
    );
}

#[test]
fn pg_state_cursor_roundtrip_upserts_per_bank_and_peer() {
    let _guard = lock_pg_test();
    let bank = unique("state-cursor");
    let s = open_pg();

    assert!(s.cursor_get(&bank, "push").expect("get").is_none());
    s.cursor_set(&bank, "push", "tok-1").expect("set");
    s.cursor_set(&bank, "pull", "tok-2").expect("set");
    s.cursor_set(&bank, "push", "tok-3").expect("set");
    assert_eq!(
        s.cursor_get(&bank, "push").expect("get"),
        Some("tok-3".into()),
        "a second set replaces the token"
    );
    assert_eq!(
        s.cursor_get(&bank, "pull").expect("get"),
        Some("tok-2".into())
    );
    assert!(
        s.cursor_get(&unique("state-cursor-other"), "push")
            .expect("get")
            .is_none(),
        "cursors are bank scoped"
    );
}

#[test]
fn pg_state_high_water_starts_empty_and_never_moves_backwards() {
    let _guard = lock_pg_test();
    let bank = unique("state-hw");
    let other = unique("state-hw-other");
    let s = open_pg();

    assert!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get")
            .is_none()
    );
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-01T00:00:02Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-01T00:00:02Z".into())
    );
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-01T00:00:01Z")
        .expect("set a lower stamp");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-01T00:00:02Z".into()),
        "a lower stamp never lowers the mark"
    );
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-01T00:00:03Z")
        .expect("set a higher stamp");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-01T00:00:03Z".into()),
        "a higher stamp raises the mark"
    );
    assert!(
        s.device_high_water_get(&bank, "dev-pg-other", "item")
            .expect("get")
            .is_none(),
        "each device keeps its own mark"
    );
    assert!(
        s.device_high_water_get(&other, "dev-pg", "item")
            .expect("get")
            .is_none(),
        "each bank keeps its own mark"
    );
}

#[test]
fn pg_state_high_water_is_independent_per_entity_kind() {
    let _guard = lock_pg_test();
    let bank = unique("state-hw-kind");
    let s = open_pg();

    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-02T00:00:00Z")
        .expect("set the item mark");
    s.device_high_water_set(&bank, "dev-pg", "triple", "2026-01-01")
        .expect("set the triple mark");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-02T00:00:00Z".into()),
        "the item mark keeps its own stamp"
    );
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "triple")
            .expect("get"),
        Some("2026-01-01".into()),
        "the triple mark is not lowered by the later item stamp"
    );
    assert!(
        s.device_high_water_get(&bank, "dev-pg", "belief")
            .expect("get")
            .is_none(),
        "an unwritten kind has no mark"
    );
}

#[test]
fn pg_state_conflict_rows_are_idempotent_and_newest_first() {
    let _guard = lock_pg_test();
    let bank = unique("state-conflict");
    let s = open_pg();

    assert_eq!(s.conflict_count(&bank).expect("count"), 0);
    s.conflict_insert(&conflict(&bank, "mut-l1", "2026-01-01T00:00:00Z"))
        .expect("insert");
    s.conflict_insert(&conflict(&bank, "mut-l1", "2026-01-01T00:00:00Z"))
        .expect("replay");
    assert_eq!(
        s.conflict_count(&bank).expect("count"),
        1,
        "the loser id keys the row, so a replay is ignored"
    );

    s.conflict_insert(&conflict(&bank, "mut-l2", "2026-01-02T00:00:00Z"))
        .expect("insert second");
    let rows = s.conflict_list(&bank, 10).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].loser_mutation_id, "mut-l2",
        "newest created_at first"
    );
    assert_eq!(rows[1].loser_mutation_id, "mut-l1");
    assert_eq!(rows[0].winner_device_id, "dev-pg");
    assert_eq!(s.conflict_list(&bank, 1).expect("list").len(), 1);
    assert_eq!(
        s.conflict_count(&unique("state-conflict-empty"))
            .expect("count"),
        0
    );
}

#[test]
fn pg_state_dead_letter_open_reopen_ack_and_list() {
    let _guard = lock_pg_test();
    let bank = unique("state-dlq");
    let s = open_pg();

    assert_eq!(s.dead_letter_status("m-none").expect("status"), None);
    let row = dead_letter(&unique("m-bad"), &bank);
    s.dead_letter_open(&row).expect("open");
    assert_eq!(
        s.dead_letter_status(&row.sync_mutation_id).expect("status"),
        Some(DeadLetterStatus::Open)
    );
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::Open)
            .expect("count"),
        1
    );
    assert_eq!(
        s.dead_letter_list(&bank, DeadLetterStatus::Open, 10)
            .expect("list")
            .len(),
        1
    );

    let mut reopened = row.clone();
    reopened.reason_code = "untrusted".into();
    reopened.last_error = Some("boom".into());
    s.dead_letter_open(&reopened).expect("reopen");
    let listed = s
        .dead_letter_list(&bank, DeadLetterStatus::Open, 10)
        .expect("list");
    assert_eq!(listed[0].reason_code, "untrusted");
    assert_eq!(listed[0].last_error.as_deref(), Some("boom"));
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::Open)
            .expect("count"),
        1,
        "a reopen updates the row instead of adding one"
    );

    assert!(
        s.dead_letter_ack_skip(&row.sync_mutation_id, "2026-03-03T00:00:00Z")
            .expect("ack")
    );
    assert!(
        !s.dead_letter_ack_skip(&row.sync_mutation_id, "2026-03-04T00:00:00Z")
            .expect("ack"),
        "a second ack changes nothing"
    );
    assert_eq!(
        s.dead_letter_status(&row.sync_mutation_id).expect("status"),
        Some(DeadLetterStatus::AckedSkip)
    );
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::AckedSkip)
            .expect("count"),
        1
    );
    assert_eq!(
        s.dead_letter_count(&bank, DeadLetterStatus::Open)
            .expect("count"),
        0
    );
    assert_eq!(
        s.dead_letter_list(&bank, DeadLetterStatus::AckedSkip, 10)
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        s.dead_letter_list(&unique("state-dlq-empty"), DeadLetterStatus::Open, 10)
            .expect("list")
            .len(),
        0
    );
}

#[test]
fn pg_state_erasure_probes_read_their_tables() {
    let _guard = lock_pg_test();
    let bank = unique("state-erase");
    let s = open_pg();

    let live = unique("subject-live");
    let gone = unique("subject-gone");
    assert!(!s.subject_is_shredded(&live).expect("probe"));
    s.upsert_subject_key_ref(&gone, "shredded", "shredded")
        .expect("shred");
    assert!(s.subject_is_shredded(&gone).expect("probe"));

    assert!(!s.persona_context_erased(&bank).expect("probe"));
    s.test_execute(&format!(
        "INSERT INTO persona_erase_tombstones (bank_id, erased_at) \
         VALUES ('{bank}', '2026-01-01T00:00:00Z')"
    ))
    .expect("tombstone");
    assert!(s.persona_context_erased(&bank).expect("probe"));
    assert!(
        !s.persona_context_erased(&unique("state-erase-other"))
            .expect("probe"),
        "the probe is bank scoped"
    );
}
