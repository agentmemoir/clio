// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compliance-erasure tests of the bank-level
//! persona erase tombstone lifecycle: erasure eligibility for
//! banks whose only persona content is a sync-derived copy, the tombstone the
//! erase writes, and the fail-closed `ErasedSubject` refusal of later persona
//! writes into an erased bank (including the Postgres advisory-lock
//! serialization between an in-flight persona write and the erase).
//!
//! ## Owns
//! - The persona-only-bank erase-proceeds scenarios (live persona rows) with
//!   tombstone and post-erase refusal assertions (SQLite and Postgres)
//! - The sync-copy-only bank scenarios: a journal persona payload or a
//!   dead-letter persona envelope alone makes the bank erasable (no
//!   `not_found`), the copies are blanked, and the tombstone is written
//! - The Postgres concurrency tests proving a persona write blocks behind the
//!   `persona-erase` bank lock and fails closed on a committing tombstone,
//!   and that a persona write serializes behind the `persona-write` slot lock
//!
//! ## Does not own
//! - The persona derived-copy purge assertions (`erase_persona_tests`)
//! - The erasure transaction under test (`sqlite_erase` / `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to the bank-level tombstone lifecycle assertions should normally
//! require changes here. Changes to the derived-copy purge coverage should
//! require `erase_persona_tests`.
//!
//! Keep this module focused on persona-erase tombstone lifecycle coverage.
//!
//! Test-only. Never compiled into production builds.

use super::*;

use crate::persona_store::PersonaStore;
use std::time::{Duration, Instant};

/// The erase wrote the bank-level persona tombstone with the request's
/// `deleted_at`.
fn assert_persona_tombstone(rows: &[String], erased_at: &str) {
    assert_eq!(rows, &[erased_at.to_owned()], "persona erase tombstone");
}

/// End to end: a bank whose only persona content is the live persona rows
/// still erases (fail closed), writes the bank-level persona tombstone, and
/// refuse-fails every later persona write into the bank: no
/// persona context resurrection.
#[test]
fn sqlite_erase_proceeds_for_persona_only_bank() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-persona-only");
    let subject = unique("subject");
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, Some("secret stable context")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, Some("secret preference context")),
        "agent",
    )
    .unwrap();
    let request = unique("req");

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert!(outcome.item_ids.is_empty(), "{:?}", outcome.item_ids);
    assert_eq!(outcome.tombstones_written, 1);
    assert_eq!(outcome.persona_context_rows_purged, 2);
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(doc.stable[0].context.is_none());
    assert!(doc.preferences[0].context.is_none());
    assert_eq!(store.read_tombstones(&bank, &request).unwrap().len(), 1);
    assert_persona_tombstone(
        &sqlite_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = ?1",
            &bank,
        ),
        spec(&bank, &subject, &request).deleted_at,
    );
    // Fail closed: the purged bank's persona context cannot be resurrected.
    let refused = ErrorCode::ErasedSubject;
    assert_eq!(
        PersonaStore::put_stable_entry(&store, &stable_entry(&bank, None), "agent")
            .unwrap_err()
            .code,
        refused
    );
    assert_eq!(
        PersonaStore::put_preference_state(&store, &pref_entry(&bank, None), "agent")
            .unwrap_err()
            .code,
        refused
    );
}

#[test]
fn postgres_erase_proceeds_for_persona_only_bank() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-persona-only");
    let subject = unique("subject");
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, Some("secret stable context")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, Some("secret preference context")),
        "agent",
    )
    .unwrap();
    let request = unique("req");

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert!(outcome.item_ids.is_empty(), "{:?}", outcome.item_ids);
    assert_eq!(outcome.tombstones_written, 1);
    assert_eq!(outcome.persona_context_rows_purged, 2);
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(doc.stable[0].context.is_none());
    assert!(doc.preferences[0].context.is_none());
    assert_persona_tombstone(
        &pg_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = $1",
            &bank,
        ),
        spec(&bank, &subject, &request).deleted_at,
    );
    // Fail closed: the purged bank's persona context cannot be resurrected.
    let refused = ErrorCode::ErasedSubject;
    assert_eq!(
        PersonaStore::put_stable_entry(&store, &stable_entry(&bank, None), "agent")
            .unwrap_err()
            .code,
        refused
    );
    assert_eq!(
        PersonaStore::put_preference_state(&store, &pref_entry(&bank, None), "agent")
            .unwrap_err()
            .code,
        refused
    );
}

/// A bank whose only persona content is a sync journal payload erases instead
/// of returning `not_found` (a): the sealed copy is blanked and the
/// bank-level tombstone is written.
#[test]
fn sqlite_erase_journal_copy_only_bank() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(kms.clone()).unwrap();
    let bank = unique("sqlite-persona-journal");
    let subject = unique("subject");
    let sealed = sealed_journal_payload(kms.as_ref(), b"secret journal context");
    store.journal_append(&journal_row(&bank, &sealed)).unwrap();

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.persona_context_rows_purged, 1);
    let journal = store
        .journal_get(&bank, "mut-persona-journal")
        .unwrap()
        .expect("journal row survives for bookkeeping");
    assert!(journal.content_sealed.is_none());
    assert_persona_tombstone(
        &sqlite_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = ?1",
            &bank,
        ),
        spec(&bank, &subject, "req").deleted_at,
    );
}

/// Same scenario on Postgres: eligibility counts the sync journal copy, the
/// purge blanks it, and the tombstone is written (parity with SQLite).
#[test]
fn postgres_erase_journal_copy_only_bank() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = local_kms();
    let store = PostgresStore::open(url.as_str(), kms.clone()).unwrap();
    let bank = unique("pg-persona-journal");
    let subject = unique("subject");
    let sealed = sealed_journal_payload(kms.as_ref(), b"secret journal context");
    store.journal_append(&journal_row(&bank, &sealed)).unwrap();

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.persona_context_rows_purged, 1);
    let journal = store
        .journal_get(&bank, "mut-persona-journal")
        .unwrap()
        .expect("journal row survives for bookkeeping");
    assert!(journal.content_sealed.is_none());
    assert_persona_tombstone(
        &pg_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = $1",
            &bank,
        ),
        spec(&bank, &subject, "req").deleted_at,
    );
}

/// A bank whose only persona content is a quarantined dead-letter persona
/// envelope erases instead of returning `not_found` (a): the envelope is
/// blanked and the bank-level tombstone is written.
#[test]
fn sqlite_erase_dead_letter_copy_only_bank() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-persona-dlq");
    let subject = unique("subject");
    store.dead_letter_open(&dead_letter_row(&bank)).unwrap();

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.persona_context_rows_purged, 1);
    let envelopes = sqlite_column(
        &store,
        "SELECT envelope_json FROM sync_dead_letter WHERE bank_id = ?1",
        &bank,
    );
    assert_copies_context_free(&envelopes, &["secret dead letter context"]);
    assert_persona_tombstone(
        &sqlite_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = ?1",
            &bank,
        ),
        spec(&bank, &subject, "req").deleted_at,
    );
}

#[test]
fn postgres_erase_dead_letter_copy_only_bank() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-persona-dlq");
    let subject = unique("subject");
    store.dead_letter_open(&dead_letter_row(&bank)).unwrap();

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.persona_context_rows_purged, 1);
    let envelopes = pg_column(
        &store,
        "SELECT envelope_json FROM sync_dead_letter WHERE bank_id = $1",
        &bank,
    );
    assert_copies_context_free(&envelopes, &["secret dead letter context"]);
    assert_persona_tombstone(
        &pg_column(
            &store,
            "SELECT erased_at FROM persona_erase_tombstones WHERE bank_id = $1",
            &bank,
        ),
        spec(&bank, &subject, "req").deleted_at,
    );
}

/// Postgres parity for the tombstone + advisory-lock path: a persona write
/// that starts while an erase transaction holds the bank's `persona-erase`
/// advisory lock blocks until that erase commits its tombstone, then fails
/// closed with `ErasedSubject` instead of resurrecting the purged context.
#[test]
fn postgres_persona_write_blocks_on_erase_lock_then_fails_closed() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-persona-erase-lock");
    let entry = stable_entry(&bank, Some("context"));

    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let erase_url = url.as_str().to_owned();
    let erase_bank = bank.clone();
    let erase_subject = unique("subject");
    let erased_at = spec(&bank, &erase_subject, "req").deleted_at.to_owned();
    let writer = std::thread::spawn(move || {
        let mut client = crate::pg_tls::connect(&erase_url).unwrap();
        let mut tx = client.transaction().unwrap();
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtext('persona-erase:' || $1))",
            &[&erase_bank],
        )
        .unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        tx.execute(
            "INSERT INTO persona_erase_tombstones (bank_id, erased_at)
             VALUES ($1, $2)
             ON CONFLICT (bank_id) DO UPDATE SET erased_at = EXCLUDED.erased_at",
            &[&erase_bank, &erased_at],
        )
        .unwrap();
        tx.commit().unwrap();
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("erase lock holder signalled");

    // The write transaction takes the bank's persona-erase lock before its
    // tombstone probe, so it waits for the erase above and then refuses.
    let err = PersonaStore::put_stable_entry(&store, &entry, "agent").unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    writer.join().unwrap();
}

/// Postgres parity for the `persona-write` slot lock: a persona write
/// to a `(bank, key)` slot blocks while another transaction holds that slot's
/// advisory lock, so two concurrent first writes serialize instead of both
/// recording a wrong audit predecessor.
#[test]
fn postgres_persona_write_serializes_on_slot_lock() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = local_kms();
    let store = std::sync::Arc::new(PostgresStore::open(url.as_str(), kms).unwrap());
    let bank = unique("pg-persona-slot-lock");

    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder_url = url.as_str().to_owned();
    let holder_bank = bank.clone();
    let holder_key = "communication_style".to_owned();
    let writer = std::thread::spawn(move || {
        let mut client = crate::pg_tls::connect(&holder_url).unwrap();
        let mut tx = client.transaction().unwrap();
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtext('persona-write:' || $1 || ':' || $2))",
            &[&holder_bank, &holder_key],
        )
        .unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        tx.commit().unwrap();
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("slot lock holder signalled");

    let store_for_write = store.clone();
    let write_bank = bank.clone();
    let put = std::thread::spawn(move || {
        let start = Instant::now();
        PersonaStore::put_stable_entry(
            &*store_for_write,
            &stable_entry(&write_bank, Some("context")),
            "agent",
        )
        .expect("persona write after the holder releases");
        start.elapsed()
    });
    let waited = put.join().unwrap();
    writer.join().unwrap();
    assert!(
        waited >= Duration::from_millis(200),
        "persona write must serialize behind the persona-write slot lock; took {waited:?}"
    );
    let doc = PersonaStore::get_persona_document(&*store, &bank).unwrap();
    assert_eq!(doc.stable.len(), 1);
}
