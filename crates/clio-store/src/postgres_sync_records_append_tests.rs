// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the concurrent Postgres journal append
//! contract: appends from independent stores to one bank serialize, whether
//! they carry the same mutation id or different ones.
//!
//! ## Owns
//! - Concurrent `journal_append_pg` coverage over a single bank
//!
//! ## Does not own
//! - Sequential append and change-feed bodies
//!   (`postgres_sync_records_feed_tests`)
//! - SQLite append behaviour (see the SQLite sync tests)
//!
//! ## Boundary
//!
//! Tests here break when the append's per-bank serialization changes; they
//! never change it.
//!
//! Keep this module focused on concurrent journal append coverage.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::{JournalAppendOutcome, SyncJournalRow, SyncStore};
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use std::sync::{Arc, Barrier};

/// Appends per test: enough concurrent writers to reach the seq allocation.
const THREADS: usize = 4;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

fn journal_row(bank_id: &str, sync_mutation_id: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: sync_mutation_id.into(),
        bank_id: bank_id.into(),
        entity_kind: "item".into(),
        entity_id: format!("item-{sync_mutation_id}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-concurrent".into(),
        content_hash: format!("hash-{sync_mutation_id}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

/// The text of a panic payload, so a failure in a joined thread can be reported.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-text panic payload".to_owned())
}

/// Append one mutation per thread from an independently opened store, all
/// released from one barrier, and collect the outcomes in submission order.
///
/// The shared-Postgres test lock is held by the caller, so the only writer to
/// this bank is this helper; the stores are opened before the threads start so
/// only the appends are concurrent.
///
/// Every handle is joined before anything is raised. Re-raising the first
/// failure inside the loop would drop the remaining handles mid-append, and
/// those detached appends would then outlive the caller's shared-database
/// guard and the bank's lock key — a secondary failure in an unrelated test.
/// The first failure is kept and raised once every thread has been joined.
fn append_concurrently(bank_id: &str, mutations: &[String]) -> Vec<JournalAppendOutcome> {
    let barrier = Arc::new(Barrier::new(mutations.len()));
    // Every append thread is spawned before any is joined, so nothing but the
    // barrier orders the appends.
    let mut handles = Vec::with_capacity(mutations.len());
    for sync_mutation_id in mutations {
        let store = open_pg();
        let bank_id = bank_id.to_owned();
        let sync_mutation_id = sync_mutation_id.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let row = journal_row(&bank_id, &sync_mutation_id);
            barrier.wait();
            super::journal_append_pg(&store, &row)
        }));
    }
    let mut failure: Option<String> = None;
    let mut outcomes = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.join() {
            Ok(Ok(outcome)) => outcomes.push(outcome),
            Ok(Err(err)) => {
                failure.get_or_insert_with(|| format!("append failed: {err:?}"));
            }
            Err(payload) => {
                failure.get_or_insert_with(|| {
                    format!("append thread panicked: {}", panic_message(&payload))
                });
            }
        }
    }
    if let Some(failure) = failure {
        panic!("{failure}");
    }
    outcomes
}

/// The same mutation appended concurrently from independent stores: every
/// append succeeds, exactly one reports `inserted`, and the journal keeps one
/// row. A concurrent duplicate must report the journalled seq, not an error.
#[test]
fn pg_journal_append_same_mutation_concurrently_lands_once() {
    let _guard = lock_pg_test();
    let reader = open_pg();
    let bank = unique("pg-journal-same");
    let mutation = unique("mut-same");
    let mutations = vec![mutation.clone(); THREADS];

    let outcomes = append_concurrently(&bank, &mutations);

    let inserted: Vec<i64> = outcomes
        .iter()
        .filter(|outcome| outcome.inserted)
        .map(|outcome| outcome.seq)
        .collect();
    assert_eq!(
        inserted.len(),
        1,
        "exactly one concurrent append inserts: {outcomes:?}"
    );
    let seq = inserted[0];
    assert!(
        outcomes.iter().all(|outcome| outcome.seq == seq),
        "every append reports the journalled seq {seq}: {outcomes:?}"
    );

    let rows = reader
        .journal_list_since(&bank, 0, 100)
        .expect("journal list");
    assert_eq!(rows.len(), 1, "one journal row for the mutation: {rows:?}");
    assert_eq!(rows[0].sync_mutation_id, mutation);
}

/// Different mutations appended concurrently from independent stores: every
/// append succeeds, and the per-bank seq allocation hands each one a distinct
/// position instead of colliding on the `(bank_id, seq)` unique index.
#[test]
fn pg_journal_append_distinct_mutations_concurrently_get_distinct_seqs() {
    let _guard = lock_pg_test();
    let reader = open_pg();
    let bank = unique("pg-journal-distinct");
    let mutations: Vec<String> = (0..THREADS).map(|_| unique("mut-distinct")).collect();

    let outcomes = append_concurrently(&bank, &mutations);

    assert!(
        outcomes.iter().all(|outcome| outcome.inserted),
        "every distinct mutation inserts: {outcomes:?}"
    );
    let mut seqs: Vec<i64> = outcomes.iter().map(|outcome| outcome.seq).collect();
    seqs.sort_unstable();
    // The bank starts empty, so a serialized allocation fills 1..=THREADS with
    // no gap and no repeat.
    let expected: Vec<i64> = (1..=i64::try_from(THREADS).expect("thread count")).collect();
    assert_eq!(seqs, expected, "seqs are distinct and gap-free");
    assert_eq!(
        reader.journal_max_seq(&bank).expect("journal max seq"),
        i64::try_from(THREADS).expect("thread count")
    );
}
