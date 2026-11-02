// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the journal append locking and duplicate
//! lookup helpers on their own: the per-bank advisory lock and the stored-`seq`
//! probe are taken here inside a caller-owned transaction, exactly in the order
//! the append takes them, without appending concurrently.
//!
//! ## Owns
//! - `lock_journal_append` and `journal_seq_probe` against a fresh bank
//!
//! ## Does not own
//! - The concurrent append contract (`postgres_sync_records_append_tests`)
//! - Sequential append and change-feed bodies
//!   (`postgres_sync_records_feed_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the lock keying or the probe SQL changes; they never
//! change it.
//!
//! Keep this module focused on the append lock and its duplicate probe.

use super::{journal_seq_probe, lock_journal_append};
use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::{SyncJournalRow, SyncStore};
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

/// One journalled mutation for a fresh bank, in the shape the append stores.
fn journal_row(bank_id: &str, sync_mutation_id: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: sync_mutation_id.into(),
        bank_id: bank_id.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{sync_mutation_id}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-journal-locks".into(),
        content_hash: format!("hash-{sync_mutation_id}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

/// The probe's answer for `(bank, mutation)`, read in a transaction that takes
/// the per-bank append lock first.
///
/// This is the ordering the append uses, so a probe that only works unlocked
/// would not be testing the real call site. The transaction is dropped rather
/// than committed: the advisory lock is transaction-scoped, so dropping it
/// releases the lock and leaves no row behind.
fn probe_under_lock(store: &PostgresStore, bank_id: &str, sync_mutation_id: &str) -> Option<i32> {
    store
        .with_client(|client| {
            let mut tx = client.transaction().expect("journal probe transaction");
            lock_journal_append(&mut tx, bank_id)?;
            journal_seq_probe(&mut tx, bank_id, sync_mutation_id)
        })
        .expect("journal probe")
}

/// Taking the per-bank append lock and probing a mutation the journal has never
/// stored reports no seq: the lock is acquirable and the probe's miss is a
/// `None` rather than an error.
#[test]
fn pg_journal_append_lock_and_probe_report_no_seq_for_an_unseen_mutation() {
    let _guard = lock_pg_test();
    let store = open_pg();
    let bank = unique("pg-journal-probe-unseen");
    let mutation = unique("mut-unseen");

    let probed = probe_under_lock(&store, &bank, &mutation);

    assert!(
        probed.is_none(),
        "an unjournalled mutation reports no seq: {probed:?}"
    );
}

/// A mutation appended through the store API is reported by the probe with the
/// seq that append committed, read back under the same per-bank lock.
#[test]
fn pg_journal_append_lock_and_probe_report_the_appended_seq() {
    let _guard = lock_pg_test();
    let store = open_pg();
    let bank = unique("pg-journal-probe-stored");
    let mutation = unique("mut-stored");

    let outcome = store
        .journal_append(&journal_row(&bank, &mutation))
        .expect("journal append");
    assert!(outcome.inserted, "the first append inserts: {outcome:?}");
    let stored = i32::try_from(outcome.seq).expect("seq fits the stored column");

    let probed = probe_under_lock(&store, &bank, &mutation);

    assert_eq!(
        probed,
        Some(stored),
        "the probe reports the seq the append committed"
    );
}
