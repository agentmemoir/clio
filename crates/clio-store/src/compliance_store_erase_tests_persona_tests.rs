// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compliance-erasure tests of the
//! persona-context lifecycle: the erasure purges the bank-scoped sealed
//! persona context and every persona-derived copy of it (the sync journal
//! payload and the dead-letter envelope).
//!
//! ## Owns
//! - End-to-end erasure assertions for the persona context and its
//!   sync-derived copies on SQLite and Postgres
//!
//! ## Does not own
//! - The erasure transaction (`sqlite_erase` / `postgres_erase`)
//! - Erasure-test fixtures (`erase_test_support`)
//! - The bank-level tombstone lifecycle scenarios (`erase_tests_persona_tests_tombstone_tests`)
//!
//! ## Boundary
//!
//! Changes to the persona-erasure lifecycle assertions should normally require
//! changes here. Changes to the erasure transaction or shared fixtures should
//! not require changes here.
//!
//! Keep this module focused on persona-erasure lifecycle coverage.
//!
//! Test-only. Never compiled into production builds.

use super::*;

use crate::persona_store::PersonaStore;
use crate::{
    CiphertextEnvelope, DeadLetterStatus, DekProvider, SyncDeadLetterRow, SyncJournalRow, SyncStore,
};
use clio_types::{AmError, PersonaPreference, PersonaStableEntry};

/// Raw contexts written into the persona records and derived sync copies.
const RAW_CONTEXTS: [&str; 4] = [
    "secret stable context",
    "prior preference context",
    "replacement preference context",
    "secret journal context",
];

fn stable_entry(bank: &str, context: Option<&str>) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: bank.into(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.5),
        source: "explicit".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: "2026-09-10".into(),
        valid_until: None,
    }
}

fn pref_entry(bank: &str, context: Option<&str>) -> PersonaPreference {
    PersonaPreference {
        bank_id: bank.into(),
        key: "brevity_affinity".into(),
        value: 0.5,
        trend: 0.5,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: "2026-09-10".into(),
    }
}

fn internal(e: impl std::fmt::Display) -> AmError {
    AmError::new(ErrorCode::Internal, e.to_string())
}

/// Seal the journal payload the way the append path does: under
/// `item_subject_id`, which falls back to the literal `sync-journal` for
/// persona payloads — a DEK erasure never destroys.
fn sealed_journal_payload(kms: &dyn DekProvider, plaintext: &[u8]) -> String {
    kms.encrypt("sync-journal", plaintext)
        .expect("seal journal payload")
        .to_storage_string()
}

fn journal_row(bank: &str, sealed: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: "mut-persona-journal".into(),
        bank_id: bank.into(),
        entity_kind: "persona".into(),
        entity_id: "persona-stable:communication_style".into(),
        op: "upsert".into(),
        transaction_time: "2026-09-10T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: "hash-persona-journal".into(),
        content_sealed: Some(sealed.into()),
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-09-10T00:00:00Z".into(),
    }
}

/// A persona dead-letter whose envelope stores the whole mutation verbatim,
/// including the readable context string (as the server built it).
fn dead_letter_row(bank: &str) -> SyncDeadLetterRow {
    let id = unique("mut-persona-dlq");
    let envelope = serde_json::json!({
        "sync_mutation_id": id,
        "bank": bank,
        "entity_kind": "persona",
        "entity_id": "persona-preference:brevity_affinity",
        "op": "upsert",
        "transaction_time": "2026-09-10T00:00:00Z",
        "device_id": "dev-a",
        "content": {"mode": "plain", "data": {"context": "secret dead letter context"}},
        "content_hash": "hash-persona-dlq"
    })
    .to_string();
    SyncDeadLetterRow {
        sync_mutation_id: id,
        bank_id: bank.into(),
        reason_code: "erased_subject".into(),
        detail: "erased subject resurrection attempt".into(),
        envelope_json: envelope,
        status: DeadLetterStatus::Open,
        first_seen: "2026-09-10T00:00:00Z".into(),
        last_error: None,
        updated_at: None,
    }
}

fn sqlite_column(store: &SqliteStore, sql: &str, bank: &str) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(sql).map_err(internal)?;
            let rows = stmt
                .query_map(rusqlite::params![bank], |r| r.get::<_, String>(0))
                .map_err(internal)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(internal)
        })
        .expect("sqlite column")
}

fn pg_column(store: &PostgresStore, sql: &str, bank: &str) -> Vec<String> {
    store
        .with_client(|client| {
            let rows = client.query(sql, &[&bank]).map_err(internal)?;
            Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
        })
        .expect("pg column")
}

/// No stored copy may keep a sealed body or the raw erased context.
fn assert_copies_context_free(rows: &[String], raw_contexts: &[&str]) {
    for row in rows {
        assert!(!row.contains("cipher"), "copy kept a sealed body: {row}");
        for raw in raw_contexts {
            assert!(!row.contains(raw), "copy leaked {raw:?}: {row}");
        }
    }
}

/// Seed the persona records (including a preference replacement) and the two
/// sync-derived copies; returns the sealed journal payload.
fn seed_persona_and_sync<S: Store + PersonaStore + SyncStore>(
    store: &S,
    kms: &dyn DekProvider,
    bank: &str,
    subject: &str,
) -> String {
    let (_item_a, _parent) = seed(store, bank, subject);
    PersonaStore::put_stable_entry(store, &stable_entry(bank, Some(RAW_CONTEXTS[0])), "agent")
        .unwrap();
    PersonaStore::put_preference_state(store, &pref_entry(bank, Some(RAW_CONTEXTS[1])), "agent")
        .unwrap();
    PersonaStore::put_preference_state(store, &pref_entry(bank, Some(RAW_CONTEXTS[2])), "agent")
        .unwrap();
    let sealed = sealed_journal_payload(kms, b"secret journal context");
    store.journal_append(&journal_row(bank, &sealed)).unwrap();
    store.dead_letter_open(&dead_letter_row(bank)).unwrap();
    sealed
}

/// Post-erase assertions shared by both backends: persona reads lose the
/// context, the persona write handle resolves through `audit_trail` with
/// cipher-free telemetry, no audit detail carries the raw text, and neither
/// sync-derived copy survives.
fn assert_erasure_clean<S: Store + PersonaStore + SyncStore>(
    store: &S,
    bank: &str,
    audit_details: &[String],
    dlq_envelopes: &[String],
) {
    let doc = PersonaStore::get_persona_document(store, bank).unwrap();
    assert!(doc.stable[0].context.is_none());
    assert!(doc.preferences[0].context.is_none());
    // The persona write handle is not a ledger item; `audit_trail` must still
    // resolve it, with cipher- and context-free telemetry.
    let trail = store
        .audit_trail(bank, "persona-preference:brevity_affinity")
        .expect("persona write handle resolves through audit_trail");
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "persona_put_preference"),
        "persona telemetry reachable: {:?}",
        trail
            .events
            .iter()
            .map(|e| &e.operation)
            .collect::<Vec<_>>()
    );
    let telemetry: Vec<String> = trail.events.iter().map(|e| e.detail.to_string()).collect();
    assert_copies_context_free(
        &telemetry,
        &["prior preference context", "replacement preference context"],
    );
    assert_copies_context_free(audit_details, &RAW_CONTEXTS);
    let journal = store
        .journal_get(bank, "mut-persona-journal")
        .unwrap()
        .expect("journal row survives for bookkeeping");
    assert!(
        journal.content_sealed.is_none(),
        "journal copy survived: {:?}",
        journal.content_sealed
    );
    assert_copies_context_free(dlq_envelopes, &["secret dead letter context"]);
}

/// End to end: erasure destroys every persona-derived copy carrying the
/// subject's context, not just the live persona rows. A
/// preference replacement leaves a cipher-free audit row; the sync journal
/// seals persona payloads under the `sync-journal` DEK erasure never destroys;
/// the dead-letter quarantine stores the whole persona mutation verbatim.
#[test]
fn sqlite_erase_destroys_persona_derived_sync_copies() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(kms.clone()).unwrap();
    let bank = unique("sqlite-persona-derived");
    let subject = unique("subject");
    let sealed = seed_persona_and_sync(&store, kms.as_ref(), &bank, &subject);
    // The sealed journal copy decrypts before the erase: this is the defect.
    assert_eq!(
        kms.decrypt(&CiphertextEnvelope::from_storage_string(&sealed).unwrap())
            .unwrap(),
        b"secret journal context"
    );

    let request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert!(outcome.persona_context_rows_purged >= 2);
    let audits = sqlite_column(
        &store,
        "SELECT detail_json FROM audit_events WHERE bank_id = ?1",
        &bank,
    );
    let dlq = sqlite_column(
        &store,
        "SELECT envelope_json FROM sync_dead_letter WHERE bank_id = ?1",
        &bank,
    );
    assert_erasure_clean(&store, &bank, &audits, &dlq);
}

#[test]
fn postgres_erase_destroys_persona_derived_sync_copies() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = local_kms();
    let store = PostgresStore::open(url.as_str(), kms.clone()).unwrap();
    let bank = unique("pg-persona-derived");
    let subject = unique("subject");
    let sealed = seed_persona_and_sync(&store, kms.as_ref(), &bank, &subject);
    assert_eq!(
        kms.decrypt(&CiphertextEnvelope::from_storage_string(&sealed).unwrap())
            .unwrap(),
        b"secret journal context"
    );

    let request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert!(outcome.persona_context_rows_purged >= 2);
    let audits = pg_column(
        &store,
        "SELECT detail_json FROM audit_events WHERE bank_id = $1",
        &bank,
    );
    let dlq = pg_column(
        &store,
        "SELECT envelope_json FROM sync_dead_letter WHERE bank_id = $1",
        &bank,
    );
    assert_erasure_clean(&store, &bank, &audits, &dlq);
}

/// End to end: erasure renders the bank's persona context unreadable
/// Persona context is sealed under the bank DEK, so the verified
/// erasure purges the sealed `context_cipher` columns; reads keep working
/// with `context: None`.
#[test]
fn sqlite_erase_purges_persona_context() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-persona-erase");
    let subject = unique("subject");
    let (_item_a, _parent) = seed(&store, &bank, &subject);
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
    assert!(outcome.persona_context_rows_purged >= 2);
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(doc.stable[0].context.is_none());
    assert!(doc.preferences[0].context.is_none());
    assert!(
        PersonaStore::stable_history(&store, &bank, "communication_style")
            .unwrap()
            .iter()
            .all(|e| e.context.is_none())
    );
}

#[test]
fn postgres_erase_purges_persona_context() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-persona-erase");
    let subject = unique("subject");
    let (_item_a, _parent) = seed(&store, &bank, &subject);
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
    assert!(outcome.persona_context_rows_purged >= 2);
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(doc.stable[0].context.is_none());
    assert!(doc.preferences[0].context.is_none());
    assert!(
        PersonaStore::stable_history(&store, &bank, "communication_style")
            .unwrap()
            .iter()
            .all(|e| e.context.is_none())
    );
}

#[cfg(test)]
#[path = "compliance_store_erase_tests_persona_tests_tombstone_tests.rs"]
mod tombstone_tests;
