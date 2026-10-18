// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-immutability guard tests: item
//! updates and discrete corrections must never re-point a row to another
//! subject, and a stored row with a NULL `subject_id` must fail with a named
//! error instead of a generic driver error or panic.
//!
//! ## Owns
//! - SQLite + Postgres parity scenarios for the update and correction subject
//!   guards (NULL stored subject, same-subject success, move refusal)
//!
//! ## Does not own
//! - The guard implementations themselves (`item_update`, `sqlite_correct`,
//!   `postgres_correct`)
//! - The named subject-immutable error (`item_persist`)
//!
//! ## Boundary
//!
//! Changes to the subject-guard behavior should normally update tests here.
//! Changes to unrelated update/correct coverage should not require changes
//! here.
//!
//! Test-only. Never compiled into production builds.

use crate::audit_model::CorrectionSpec;
use crate::dek::{DekProvider, LocalDevKms};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

fn item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"port": 8080})),
        gist: Some("service listens on 8080".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn correction_spec(prior: &MemoryItem, new_id: &str, subject: &str) -> CorrectionSpec {
    let ts = "2024-06-01T00:00:00Z";
    let mut new = prior.clone();
    new.id = new_id.to_owned();
    new.subject_id = subject.to_owned();
    new.gist = Some("corrected gist".into());
    new.snapshot = Some(json!({"port": 8081}));
    new.valid_from = ts.into();
    new.valid_until = None;
    new.tx_from = ts.into();
    new.tx_until = None;
    new.created_at = ts.into();
    new.updated_at = ts.into();
    CorrectionSpec {
        bank_id: prior.bank_id.clone(),
        prior_item_id: prior.id.clone(),
        new_item: new,
        reason: "wrong detail".into(),
        actor: "tester".into(),
        timestamp: ts.into(),
    }
}

fn assert_prior_still_open(store: &dyn Store, bank: &str, id: &str) {
    let prior = store
        .get_memory_item(bank, id)
        .expect("prior readable")
        .expect("prior exists");
    assert!(
        prior.tx_until.is_none(),
        "prior row must remain open after a rejected correction: {prior:?}"
    );
}

/// Raw `tx_until IS NULL` check for rows whose subject was NULLed by the
/// test (those rows cannot be opened through `get_memory_item`).
fn assert_prior_open_sqlite(store: &SqliteStore, bank: &str, id: &str) {
    let open: i64 = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM items WHERE id = ?1 AND bank_id = ?2 AND tx_until IS NULL",
                [id, bank],
                |r| r.get(0),
            )
            .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap();
    assert_eq!(open, 1, "prior row must remain open after rejection");
}

/// Postgres twin of [`assert_prior_open_sqlite`].
fn assert_prior_open_pg(store: &PostgresStore, bank: &str, id: &str) {
    let open: i64 = store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT COUNT(*) FROM items WHERE id = $1 AND bank_id = $2 \
                     AND tx_until IS NULL",
                    &[&id, &bank],
                )
                .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
                .map(|row| row.get(0))
        })
        .unwrap();
    assert_eq!(open, 1, "prior row must remain open after rejection");
}

fn update_revised(same: &MemoryItem) -> MemoryItem {
    let mut upd = same.clone();
    upd.gist = Some("revised gist".into());
    upd.updated_at = "2024-02-01T00:00:00Z".into();
    upd
}

// -- SQLite ----------------------------------------------------------------

#[test]
fn sqlite_update_null_stored_subject_is_named_error() {
    let store = SqliteStore::open_in_memory(kms()).unwrap();
    let bank = unique("sg-b");
    let subject = unique("sg-s");
    let id = unique("sg-null");
    store
        .create_memory_item(&item(&id, &bank, &subject), "tester")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET subject_id = NULL WHERE id = '{id}' AND bank_id = '{bank}'"
        ))
        .unwrap();
    let err = store
        .update_memory_item(&update_revised(&item(&id, &bank, &subject)), "tester")
        .unwrap_err();
    // Named ConfigCorrupt error, not a generic driver-mapped Internal error.
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(err.message.contains("NULL subject_id"), "{err}");
}

#[test]
fn sqlite_update_same_subject_still_succeeds_and_move_refused() {
    let store = SqliteStore::open_in_memory(kms()).unwrap();
    let bank = unique("sg-b");
    let subject = unique("sg-s");
    let id = unique("sg-ok");
    store
        .create_memory_item(&item(&id, &bank, &subject), "tester")
        .unwrap();
    store
        .update_memory_item(&update_revised(&item(&id, &bank, &subject)), "tester")
        .unwrap();
    assert_eq!(store.get_gist(&bank, &id).unwrap().unwrap(), "revised gist");
    let mut moved = update_revised(&item(&id, &bank, &subject));
    moved.subject_id = "other-subject".into();
    let err = store.update_memory_item(&moved, "tester").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert!(err.message.contains("cannot change subject_id"), "{err}");
}

#[test]
fn sqlite_correct_subject_move_refused_and_prior_stays_open() {
    let store = SqliteStore::open_in_memory(kms()).unwrap();
    let bank = unique("sg-cb");
    let subject = unique("sg-cs");
    let prior = item(&unique("sg-c1"), &bank, &subject);
    store.create_memory_item(&prior, "tester").unwrap();
    // Move to another subject must be refused before any mutation.
    let moved = correction_spec(&prior, &unique("sg-cnew"), "other-subject");
    let err = store.correct_item(&moved).unwrap_err();
    assert_eq!(err, crate::item_persist::subject_immutable_err(&prior.id));
    assert_prior_still_open(&store, &bank, &prior.id);
    // A NULL stored subject also mismatches and must be rejected.
    let prior_null = item(&unique("sg-c2"), &bank, &subject);
    store.create_memory_item(&prior_null, "tester").unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET subject_id = NULL WHERE id = '{}' AND bank_id = '{bank}'",
            prior_null.id
        ))
        .unwrap();
    let moved_null = correction_spec(&prior_null, &unique("sg-cnew"), "other-subject");
    let err = store.correct_item(&moved_null).unwrap_err();
    assert_eq!(
        err,
        crate::item_persist::subject_immutable_err(&prior_null.id)
    );
    assert_prior_open_sqlite(&store, &bank, &prior_null.id);
    // Same-subject correction still succeeds and closes the prior row.
    let same = correction_spec(&prior, &unique("sg-csame"), &subject);
    let out = store.correct_item(&same).unwrap();
    assert!(!out.already_superseded);
    assert_eq!(out.new_item_id, same.new_item.id);
    let prior_row = store
        .get_memory_item(&bank, &prior.id)
        .unwrap()
        .expect("prior exists");
    assert!(prior_row.tx_until.is_some(), "prior closed by correction");
}

// -- Postgres parity --------------------------------------------------------

fn open_pg() -> Option<PostgresStore> {
    let url = crate::db_url::postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let store = match PostgresStore::open(url.as_str(), kms()) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("pg subject-guard test", &err);
            return None;
        }
    };
    Some(store)
}

fn cleanup_pg(store: &PostgresStore, bank: &str, subject: &str) {
    // Raw item deletes bypass `delete_item`'s queue-row cleanup, so the queue row
    // goes here too: the shared test database must not keep unowned backlog rows.
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}

#[test]
fn pg_update_null_stored_subject_is_named_error() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let bank = unique("pg-sg-b");
    let subject = unique("pg-sg-s");
    let id = unique("pg-sg-null");
    store
        .create_memory_item(&item(&id, &bank, &subject), "tester")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET subject_id = NULL WHERE id = '{id}' AND bank_id = '{bank}'"
        ))
        .unwrap();
    let err = store
        .update_memory_item(&update_revised(&item(&id, &bank, &subject)), "tester")
        .unwrap_err();
    // Named ConfigCorrupt error; must be a clean error, not a panic.
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(err.message.contains("NULL subject_id"), "{err}");
    cleanup_pg(&store, &bank, &subject);
}

#[test]
fn pg_update_same_subject_still_succeeds_and_move_refused() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let bank = unique("pg-sg-b");
    let subject = unique("pg-sg-s");
    let id = unique("pg-sg-ok");
    store
        .create_memory_item(&item(&id, &bank, &subject), "tester")
        .unwrap();
    store
        .update_memory_item(&update_revised(&item(&id, &bank, &subject)), "tester")
        .unwrap();
    assert_eq!(store.get_gist(&bank, &id).unwrap().unwrap(), "revised gist");
    let mut moved = update_revised(&item(&id, &bank, &subject));
    moved.subject_id = "other-subject".into();
    let err = store.update_memory_item(&moved, "tester").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert!(err.message.contains("cannot change subject_id"), "{err}");
    cleanup_pg(&store, &bank, &subject);
}

#[test]
fn pg_correct_subject_move_refused_and_prior_stays_open() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let bank = unique("pg-sg-cb");
    let subject = unique("pg-sg-cs");
    // Move to another subject must be refused before any mutation.
    let prior = item(&unique("pg-sg-cprior"), &bank, &subject);
    store.create_memory_item(&prior, "tester").unwrap();
    let moved = correction_spec(&prior, &unique("pg-sg-cnew"), "other-subject");
    let err = store.correct_item(&moved).unwrap_err();
    assert_eq!(err, crate::item_persist::subject_immutable_err(&prior.id));
    assert_prior_still_open(&store, &bank, &prior.id);
    // A NULL stored subject also mismatches and must be rejected (no panic).
    let prior_null = item(&unique("pg-sg-cnull"), &bank, &subject);
    store.create_memory_item(&prior_null, "tester").unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET subject_id = NULL WHERE id = '{}' AND bank_id = '{bank}'",
            prior_null.id
        ))
        .unwrap();
    let moved_null = correction_spec(&prior_null, &unique("pg-sg-cnew"), "other-subject");
    let err = store.correct_item(&moved_null).unwrap_err();
    assert_eq!(
        err,
        crate::item_persist::subject_immutable_err(&prior_null.id)
    );
    assert_prior_open_pg(&store, &bank, &prior_null.id);
    // Same-subject correction still succeeds and closes the prior row.
    let same = correction_spec(&prior, &unique("pg-sg-csame"), &subject);
    let out = store.correct_item(&same).unwrap();
    assert!(!out.already_superseded);
    assert_eq!(out.new_item_id, same.new_item.id);
    let prior_row = store
        .get_memory_item(&bank, &prior.id)
        .unwrap()
        .expect("prior exists");
    assert!(prior_row.tx_until.is_some(), "prior closed by correction");
    cleanup_pg(&store, &bank, &subject);
}
