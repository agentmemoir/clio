// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures and the SQLite side of
//! the open-edge uniqueness invariant tests: duplicate rejection, supersession
//! under the constraint, close-then-reopen, different-predicate coexistence,
//! `as_of` stability, and legacy-database upgrade behavior.
//!
//! ## Owns
//! - Shared open-edge invariant test fixtures and the SQLite cases
//!
//! ## Does not own
//! - Postgres cases (`pg_triple_open_uniq_tests`)
//! - Core assert/supersede/query happy paths (`triple_tests`, `triple_edge_tests`)
//! - The invariant implementation (`triple_invariant`, `sql/001_core.sql`)
//!
//! ## Boundary
//!
//! Changes to the open-edge constraint should normally update tests here.
//! Changes to unrelated triple behavior should not require changes here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, TimeAxis, TripleQuery,
    TripleRecord,
};
use std::sync::Arc;

pub(crate) const OPEN_UNIQ_INDEX: &str = "triples_open_uniq";

/// The previous (v10) non-unique open-edge index the v11 migration must drop.
pub(crate) const LEGACY_OPEN_INX: &str = "triples_bank_id_subject_predicate_open_both_inx";

pub(crate) fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>
}

use crate::test_unique::unique;

pub(crate) fn carrier(
    id: &str,
    bank: &str,
    valid_from: &str,
    tx_from: &str,
    object: &str,
) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"object": object})),
        gist: Some(format!("lives_in {object}")),
        valid_from: valid_from.into(),
        valid_until: None,
        tx_from: tx_from.into(),
        tx_until: None,
        created_at: tx_from.into(),
        updated_at: tx_from.into(),
        context: None,
    }
}

pub(crate) fn edge(
    id: &str,
    bank: &str,
    item_id: &str,
    predicate: &str,
    object: &str,
    valid_from: &str,
    tx_from: &str,
) -> TripleRecord {
    TripleRecord {
        context: None,
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: predicate.into(),
        object: object.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: valid_from.into(),
        valid_until: None,
        tx_from: tx_from.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: tx_from.into(),
        updated_at: None,
    }
}

/// Raw duplicate row mirroring a pre-constraint insert (driver-visible).
pub(crate) fn raw_open_triple_sql(bank: &str, item_id: &str, id: &str) -> String {
    format!(
        "INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext, \
         epistemic_kind, valid_from, tx_from, created_at) \
         VALUES ('{id}', '{bank}', '{item_id}', 'Alice', 'lives_in', 'sealed-fixture', \
         'fact', '2024-01-01', '2024-01-01', '2024-01-01')"
    )
}

// ---------------------------------------------------------------------------
// SQLite (in-memory)
// ---------------------------------------------------------------------------

#[test]
fn sqlite_duplicate_open_edge_rejected_and_supersede_succeeds() {
    let s = SqliteStore::open_in_memory(kms()).expect("sqlite");
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            "e1",
            &bank,
            "i1",
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    // Second open edge, same bank+subject+predicate: rejected before any write.
    let err = s
        .commit_triple_add(
            &carrier("i2", &bank, "2024-02-01", "2024-02-01", "Denver"),
            &edge(
                "e2",
                &bank,
                "i2",
                "lives_in",
                "Denver",
                "2024-02-01",
                "2024-02-01",
            ),
            false,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // Superseding the open edge closes the prior row and inserts in one transaction.
    let out = s
        .commit_triple_add(
            &carrier("i2", &bank, "2024-02-01", "2024-02-01", "Denver"),
            &edge(
                "e2",
                &bank,
                "i2",
                "lives_in",
                "Denver",
                "2024-02-01",
                "2024-02-01",
            ),
            true,
            "agent",
        )
        .unwrap();
    assert_eq!(out.prior_edge_ids, vec!["e1".to_owned()]);
    let prior = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(prior.valid_until.as_deref(), Some("2024-02-01"));
    assert_eq!(prior.tx_until.as_deref(), Some("2024-02-01"));
    let current = s.get_triple(&bank, "e2").unwrap().unwrap();
    assert!(current.valid_until.is_none() && current.tx_until.is_none());
}

#[test]
fn sqlite_close_then_reopen_and_different_predicate_allowed() {
    let s = SqliteStore::open_in_memory(kms()).expect("sqlite");
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            "e1",
            &bank,
            "i1",
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    s.triple_end(
        &bank,
        "Alice",
        "lives_in",
        Some("Chicago"),
        "2024-06-01",
        "2024-06-01",
    )
    .unwrap();
    // Same subject+predicate after close: a fresh open edge is allowed.
    s.commit_triple_add(
        &carrier("i2", &bank, "2024-07-01", "2024-07-01", "Denver"),
        &edge(
            "e2",
            &bank,
            "i2",
            "lives_in",
            "Denver",
            "2024-07-01",
            "2024-07-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    // Different predicate on the same subject: both edges open concurrently.
    s.commit_triple_add(
        &carrier("i3", &bank, "2024-07-01", "2024-07-01", "Boston"),
        &edge(
            "e3",
            &bank,
            "i3",
            "works_in",
            "Boston",
            "2024-07-01",
            "2024-07-01",
        ),
        false,
        "agent",
    )
    .unwrap();
}

#[test]
fn sqlite_as_of_answers_prior_belief_after_supersede() {
    let s = SqliteStore::open_in_memory(kms()).expect("sqlite");
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            "e1",
            &bank,
            "i1",
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    s.commit_triple_add(
        &carrier("i2", &bank, "2024-06-01", "2024-06-01", "Denver"),
        &edge(
            "e2",
            &bank,
            "i2",
            "lives_in",
            "Denver",
            "2024-06-01",
            "2024-06-01",
        ),
        true,
        "agent",
    )
    .unwrap();
    let current = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(
        current.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["e2"]
    );
    let then = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2024-03-01".into()),
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(
        then.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["e1"]
    );
    let believed = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2024-03-01".into()),
            time_axis: TimeAxis::Transaction,
        })
        .unwrap();
    assert_eq!(
        believed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["e1"]
    );
}

// ---------------------------------------------------------------------------
// SQLite (file-backed): existing-database upgrade and legacy duplicates
// ---------------------------------------------------------------------------

fn sqlite_index_present(store: &SqliteStore, name: &str) -> bool {
    sqlite_scalar::<i64>(
        store,
        &format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{name}'"),
    ) > 0
}

/// Scalar probe against the store connection (index presence, version).
fn sqlite_scalar<T: rusqlite::types::FromSql>(store: &SqliteStore, sql: &str) -> T {
    store
        .with_conn(|conn| {
            conn.query_row(sql, [], |r| r.get::<usize, T>(0))
                .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .expect("scalar query")
}

#[test]
fn sqlite_existing_db_gains_unique_index_on_reopen() {
    let path = std::env::temp_dir().join(format!(
        "am-openuniq-up-{}-{}.db",
        std::process::id(),
        unique("t")
    ));
    let store = SqliteStore::open(&path, kms()).expect("open");
    let bank = unique("b");
    let item = format!("{bank}-item");
    // Materialize the previous schema shape: old non-unique open index, no
    // unique index, schema_version '10', one legacy open triple (raw SQL).
    store
        .test_execute(&format!(
            "DROP INDEX IF EXISTS {OPEN_UNIQ_INDEX}; \
             CREATE INDEX IF NOT EXISTS triples_bank_id_subject_predicate_open_both_inx \
             ON triples (bank_id, subject, predicate) \
             WHERE valid_until IS NULL AND tx_until IS NULL; \
             UPDATE schema_settings SET value = '10' WHERE key = 'schema_version'; \
             INSERT INTO items (id, bank_id, kind, episodic_type, epistemic_kind, \
               valid_from, tx_from, created_at, updated_at) VALUES \
               ('{item}', '{bank}', 'episodic', 'triple', 'fact', '2024-01-01', \
               '2024-01-01', '2024-01-01', '2024-01-01'); \
             INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext, \
               epistemic_kind, valid_from, tx_from, created_at) VALUES \
               ('legacy-1', '{bank}', '{item}', 'Alice', 'lives_in', 'sealed-fixture', \
               'fact', '2024-01-01', '2024-01-01', '2024-01-01');"
        ))
        .unwrap();
    drop(store);
    // Reopening the previous-schema database must migrate it to the current
    // schema version.
    let store = SqliteStore::open(&path, kms()).expect("reopen");
    assert!(!sqlite_index_present(&store, LEGACY_OPEN_INX));
    assert!(sqlite_index_present(&store, OPEN_UNIQ_INDEX));
    let version = sqlite_scalar::<String>(
        &store,
        "SELECT value FROM schema_settings WHERE key = 'schema_version'",
    );
    assert_eq!(version, "1");
    // On the migrated database the unique index rejects a raw duplicate insert.
    let err = store
        .test_execute(&raw_open_triple_sql(&bank, &item, "dup-1"))
        .unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "{err}");
    drop(store);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_legacy_duplicate_open_edges_fail_with_named_diagnostic() {
    let path = std::env::temp_dir().join(format!(
        "am-openuniq-dup-{}-{}.db",
        std::process::id(),
        unique("t")
    ));
    let store = SqliteStore::open(&path, kms()).expect("open");
    let bank = unique("b");
    store
        .commit_triple_add(
            &carrier("i1", &bank, "2024-01-01", "2024-01-01", "Chicago"),
            &edge(
                "e1",
                &bank,
                "i1",
                "lives_in",
                "Chicago",
                "2024-01-01",
                "2024-01-01",
            ),
            false,
            "agent",
        )
        .unwrap();
    store
        .test_execute(&format!("DROP INDEX IF EXISTS {OPEN_UNIQ_INDEX};"))
        .unwrap();
    store
        .test_execute(&raw_open_triple_sql(&bank, "i1", "dup-1"))
        .unwrap();
    drop(store);
    let Err(err) = SqliteStore::open(&path, kms()) else {
        panic!("legacy duplicate must fail store open");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    let msg = err.to_string();
    assert!(msg.contains("duplicate open triples"), "{msg}");
    assert!(msg.contains("bank_id=`"), "{msg}");
    assert!(msg.contains("operator-confirmed"), "{msg}");
    // Operator-confirmed repair: close the extra open edge (never DELETE), retry.
    let conn = rusqlite::Connection::open(&path).expect("repair connection");
    conn.execute(
        "UPDATE triples SET valid_until = '2024-02-01', tx_until = '2024-02-01' \
         WHERE id = 'dup-1' AND bank_id = ?1",
        [bank.as_str()],
    )
    .expect("close duplicate");
    drop(conn);
    let store = SqliteStore::open(&path, kms()).expect("reopen after repair");
    assert!(sqlite_index_present(&store, OPEN_UNIQ_INDEX));
    drop(store);
    let _ = std::fs::remove_file(&path);
}
