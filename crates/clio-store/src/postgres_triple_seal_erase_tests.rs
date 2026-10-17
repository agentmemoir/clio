// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres parity tests of a triple's sealed
//! value, of what a verified compliance erasure must leave behind, and of the
//! legacy-shape open refusal on a pre-fix database.
//!
//! ## Owns
//! - Postgres parity for "the asserted value is sealed at rest", "`triple_add`
//!   then `erase_subject` leaves no readable copy", "the sync-journal and
//!   dead-letter payload copies are blanked", and "a legacy-shaped store
//!   refuses to open and keeps its rows"
//!
//! ## Does not own
//! - The legacy-shape open guard on a pre-fix database (`pg_triple_reshape_tests.rs`)
//! - SQLite behaviour (see `triple_seal_erase_tests`, `triple_reshape_tests`)
//! - Postgres triple query semantics (see `pg_triple_tests`)
//!
//! ## Boundary
//!
//! Changes to the Postgres triple value contract should normally update tests
//! here. Changes to SQLite-only behaviour should not require changes here.

use crate::compliance_store::{ComplianceStore, SubjectEraseSpec};
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::migrate::IntoAmError;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::sync_store::{SyncDeadLetterRow, SyncJournalRow, SyncStore};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem, TripleRecord};
use std::sync::Arc;

/// The secret a triple value carries, and the needle the raw-row checks look for.
pub(crate) const SECRET: &str = "hunter2Correct";

use crate::test_unique::unique;

/// A per-test data-subject id.
///
/// Erasure is subject-global: `subject_keys` is not bank-scoped, and destroying
/// one DEK permanently refuses writes for that subject, so no test may shred a
/// subject id another test still needs.
pub(crate) fn unique_subject() -> String {
    unique("pg-seal-subj")
}

/// A per-test item/edge id pair. Item ids are deployment-unique, so the shared
/// Postgres database needs a fresh id per test.
pub(crate) struct Ids {
    pub(crate) item: String,
    pub(crate) edge: String,
}

pub(crate) fn unique_ids() -> Ids {
    let n = unique("pg-seal");
    Ids {
        item: format!("i-{n}"),
        edge: format!("e-{n}"),
    }
}

pub(crate) fn url() -> String {
    postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"))
}

pub(crate) fn open_pg() -> Option<PostgresStore> {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    match PostgresStore::open(&url(), kms) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres triple seal tests", &err);
            None
        }
    }
}

pub(crate) fn carrier(bank: &str, subject: &str, ids: &Ids) -> MemoryItem {
    MemoryItem {
        id: ids.item.clone(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"object": SECRET})),
        gist: Some(format!("db password is {SECRET}")),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
        context: None,
    }
}

pub(crate) fn edge(bank: &str, ids: &Ids) -> TripleRecord {
    TripleRecord {
        id: ids.edge.clone(),
        bank_id: bank.into(),
        item_id: ids.item.clone(),
        subject: "svc:db".into(),
        predicate: "password_is".into(),
        object: SECRET.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

fn erase_spec<'a>(bank: &'a str, subject: &'a str, request: &'a str) -> SubjectEraseSpec<'a> {
    // A fresh salt per process: no test asserts the tombstone hash value.
    static SALT: std::sync::LazyLock<[u8; 16]> = std::sync::LazyLock::new(rand::random);
    SubjectEraseSpec {
        bank_id: bank,
        subject_id: subject,
        request_id: request,
        legal_basis: "GDPR_ART_17",
        reason_code: "COMPLIANCE_CRYPTO_SHRED",
        deleted_at: "2026-09-17T11:00:00Z",
        salt: &SALT[..],
    }
}

/// A journal row shaped as the sync client packages a closed triple: an
/// `invalidate` mutation whose payload is the triple's plaintext SPO, sealed
/// under the node-local journal DEK (a DEK no subject erasure destroys).
fn journal_row(bank: &str, ids: &Ids) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: format!("mut-{}", ids.edge),
        bank_id: bank.into(),
        entity_kind: "triple".into(),
        entity_id: ids.edge.clone(),
        op: "invalidate".into(),
        transaction_time: "2024-02-01T00:00:00Z".into(),
        device_id: "dev-1".into(),
        content_hash: "hash-1".into(),
        content_sealed: Some(format!("sealed-payload-{SECRET}")),
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2024-02-01T00:00:00Z".into(),
    }
}

fn dead_letter_row(bank: &str, ids: &Ids) -> SyncDeadLetterRow {
    SyncDeadLetterRow {
        sync_mutation_id: format!("dlq-{}", ids.edge),
        bank_id: bank.into(),
        reason_code: "malformed".into(),
        detail: "never applied".into(),
        envelope_json: serde_json::json!({
            "entity_kind": "triple",
            "entity_id": ids.edge,
            "op": "invalidate",
            "content": {"subject": "svc:db", "predicate": "password_is", "object": SECRET},
        })
        .to_string(),
        status: crate::sync_store::DeadLetterStatus::Open,
        first_seen: "2024-02-01T00:00:00Z".into(),
        last_error: Some("boom".into()),
        updated_at: None,
    }
}

fn journal_payload(pg: &PostgresStore, bank: &str, mutation_id: &str) -> Option<String> {
    pg.journal_get(bank, mutation_id)
        .expect("journal get")
        .and_then(|row| row.content_sealed)
}

fn dead_letter_envelope(pg: &PostgresStore, bank: &str, mutation_id: &str) -> String {
    pg.dead_letter_list(bank, crate::sync_store::DeadLetterStatus::Open, 50)
        .expect("dead letter list")
        .into_iter()
        .find(|row| row.sync_mutation_id == mutation_id)
        .map(|row| row.envelope_json)
        .unwrap_or_default()
}

pub(crate) fn raw_object_column(pg: &PostgresStore, bank: &str, edge_id: &str) -> Option<String> {
    pg.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT object_ciphertext FROM triples WHERE bank_id = $1 AND id = $2",
                &[&bank, &edge_id],
            )
            .am("raw triple value")?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    })
    .expect("raw read")
}

fn raw_triple_rows(pg: &PostgresStore, bank: &str) -> i64 {
    pg.with_client(|client| {
        Ok(client
            .query_one("SELECT COUNT(*) FROM triples WHERE bank_id = $1", &[&bank])
            .am("count triple rows")?
            .get(0))
    })
    .expect("count read")
}

#[test]
fn pg_the_asserted_value_is_sealed_at_rest_and_reads_stay_exact() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("pg-seal-b");
    let subject = unique_subject();
    let ids = unique_ids();
    pg.commit_triple_add(
        &carrier(&bank, &subject, &ids),
        &edge(&bank, &ids),
        true,
        "actor",
    )
    .expect("triple add");
    let raw = raw_object_column(&pg, &bank, &ids.edge).expect("a stored value");
    assert!(
        !raw.contains(SECRET),
        "the value must be sealed at rest: {raw}"
    );
    let got = pg.get_triple(&bank, &ids.edge).unwrap().expect("edge");
    assert_eq!(got.object, SECRET);
    assert_eq!(got.subject, "svc:db");
    assert_eq!(got.predicate, "password_is");
}

#[test]
fn pg_erase_subject_leaves_no_readable_copy_of_the_triple() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("pg-seal-b");
    let subject = unique_subject();
    let ids = unique_ids();
    pg.commit_triple_add(
        &carrier(&bank, &subject, &ids),
        &edge(&bank, &ids),
        true,
        "actor",
    )
    .expect("triple add");
    let outcome = pg
        .erase_subject(&erase_spec(&bank, &subject, &unique("pg-req")))
        .expect("compliance erase");
    assert_eq!(outcome.triples_purged, 1, "the erase must report the row");
    assert_eq!(raw_triple_rows(&pg, &bank), 0, "the row must be deleted");
    assert!(pg.get_triple(&bank, &ids.edge).unwrap().is_none());
    let remaining = pg.list_triples(&bank).unwrap();
    assert!(remaining.is_empty(), "{remaining:?}");
}

#[test]
fn pg_the_erase_blanks_the_sync_journal_and_dead_letter_copies() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("pg-seal-b");
    let subject = unique_subject();
    let ids = unique_ids();
    pg.commit_triple_add(
        &carrier(&bank, &subject, &ids),
        &edge(&bank, &ids),
        true,
        "actor",
    )
    .expect("triple add");
    pg.journal_append(&journal_row(&bank, &ids))
        .expect("journal append");
    pg.dead_letter_open(&dead_letter_row(&bank, &ids))
        .expect("dead letter");
    // The pre-erase state must actually hold the plaintext, or this test would
    // pass for the wrong reason.
    assert!(
        journal_payload(&pg, &bank, &format!("mut-{}", ids.edge))
            .expect("a journal payload")
            .contains(SECRET)
    );
    assert!(dead_letter_envelope(&pg, &bank, &format!("dlq-{}", ids.edge)).contains(SECRET));

    pg.erase_subject(&erase_spec(&bank, &subject, &unique("pg-req")))
        .expect("compliance erase");

    // Both copies keep their bookkeeping rows so sync's replay and ack
    // accounting stay sane, but the payloads are gone.
    assert_eq!(
        journal_payload(&pg, &bank, &format!("mut-{}", ids.edge)),
        None,
        "journal payload must be blanked"
    );
    assert_eq!(
        pg.journal_max_seq(&bank).expect("max seq"),
        1,
        "row survives"
    );
    let envelope = dead_letter_envelope(&pg, &bank, &format!("dlq-{}", ids.edge));
    assert!(
        !envelope.contains(SECRET),
        "envelope must be blanked: {envelope}"
    );
    assert_eq!(envelope, r#"{"erased":true}"#);
    assert_eq!(
        pg.dead_letter_status(&format!("dlq-{}", ids.edge))
            .expect("status"),
        Some(crate::sync_store::DeadLetterStatus::Open),
        "the quarantine row survives"
    );
}

#[test]
fn pg_the_purge_only_touches_the_purged_triples_own_copies() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("pg-seal-b");
    let subject = unique_subject();
    let ids = unique_ids();
    pg.commit_triple_add(
        &carrier(&bank, &subject, &ids),
        &edge(&bank, &ids),
        true,
        "actor",
    )
    .expect("triple add");
    // Another triple's journal row, and a row of a different entity kind. Both
    // must survive: the purge is scoped to the ids of the triples it deleted.
    // (A `persona` row would not be a fair probe — the persona purge is
    // bank-scoped and blanks those on any verified erasure, by design.)
    let other_edge = format!("{}-other", ids.edge);
    let mut other = journal_row(&bank, &ids);
    other.sync_mutation_id = format!("mut-{other_edge}");
    other.entity_id = other_edge.clone();
    pg.journal_append(&other).expect("journal append");
    let belief_id = format!("mut-belief-{}", ids.edge);
    let mut belief = journal_row(&bank, &ids);
    belief.sync_mutation_id = belief_id.clone();
    belief.entity_kind = "belief".into();
    belief.content_sealed = Some("belief-payload".into());
    pg.journal_append(&belief).expect("journal append");
    let other_dead_id = format!("dlq-{other_edge}");
    let mut other_dead = dead_letter_row(&bank, &ids);
    other_dead.sync_mutation_id = other_dead_id.clone();
    other_dead.envelope_json = serde_json::json!({
        "entity_kind": "triple",
        "entity_id": other_edge,
        "content": {"object": "unrelated"},
    })
    .to_string();
    pg.dead_letter_open(&other_dead).expect("dead letter");

    pg.erase_subject(&erase_spec(&bank, &subject, &unique("pg-req")))
        .expect("compliance erase");

    assert_eq!(
        journal_payload(&pg, &bank, &format!("mut-{}", ids.edge)),
        None,
        "the purged triple's own journal row is blanked"
    );
    let kept = journal_payload(&pg, &bank, &format!("mut-{other_edge}"))
        .expect("another triple's row survives");
    assert!(kept.contains(SECRET), "another triple's payload stays");
    assert_eq!(
        journal_payload(&pg, &bank, &belief_id).as_deref(),
        Some("belief-payload"),
        "a different entity kind is untouched"
    );
    assert!(dead_letter_envelope(&pg, &bank, &other_dead_id).contains("unrelated"));
}

#[test]
fn pg_erasing_a_subject_with_no_triples_still_succeeds() {
    // The purge's empty short-circuit: a subject whose bank holds no triple rows
    // must still get a complete, honest erasure record.
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("pg-seal-b");
    let subject = unique_subject();
    let item = crate::erase_test_support::memory_item(&bank, &unique("i"), &subject);
    pg.create_memory_item(&item, "actor").expect("item");
    let outcome = pg
        .erase_subject(&erase_spec(&bank, &subject, &unique("pg-req")))
        .expect("compliance erase");
    assert_eq!(outcome.triples_purged, 0, "no triple rows to purge");
    assert_eq!(outcome.tombstones_written, 1);
}
