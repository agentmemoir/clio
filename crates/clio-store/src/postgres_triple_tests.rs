// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity reads of bi-temporal triples.
//!
//! ## Owns
//! - As-of and context parity reads, and the fixtures both Postgres triple test modules share
//!
//! ## Does not own
//! - Write guards and commit refusals (see `postgres_triple_guard_tests`)
//! - SQLite triple behavior (see `sqlite_triple_tests` / `sqlite_triple_edge_tests`)
//!
//! ## Boundary
//!
//! Changes to Postgres triple read SQL should normally update tests here.
//! Changes to the triple write guards should not require changes here.
//!
//! Keep this module focused on what a Postgres triple read returns.

use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ItemKind, MemoryItem, TimeAxis, TripleQuery, TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

pub(crate) fn open_pg() -> Option<PostgresStore> {
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(&url, kms) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres triple tests", &err)
            );
            return None;
        }
    };
    Some(store)
}

pub(crate) fn carrier(
    id: &str,
    bank: &str,
    object: &str,
    valid_from: &str,
    tx_from: &str,
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
        snapshot: Some(json!({"object": object})),
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
    object: &str,
    valid_from: &str,
    tx_from: &str,
) -> TripleRecord {
    TripleRecord {
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
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
        context: None,
        created_at: tx_from.into(),
        updated_at: None,
    }
}

#[test]
fn t10_postgres_as_of_parity_sample() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-trip");
    let i1 = unique("i1");
    let i2 = unique("i2");
    let e1 = unique("e1");
    let e2 = unique("e2");
    store
        .commit_triple_add(
            &carrier(&i1, &bank, "Chicago", "2020-01-01", "2024-01-01"),
            &edge(&e1, &bank, &i1, "Chicago", "2020-01-01", "2024-01-01"),
            true,
            "agent",
        )
        .unwrap();
    store
        .commit_triple_add(
            &carrier(&i2, &bank, "Boston", "2024-06-01", "2024-06-02"),
            &edge(&e2, &bank, &i2, "Boston", "2024-06-01", "2024-06-02"),
            true,
            "agent",
        )
        .unwrap();
    let prior = store.get_triple(&bank, &e1).unwrap().unwrap();
    assert_eq!(prior.valid_until.as_deref(), Some("2024-06-01"));
    assert_eq!(prior.tx_until.as_deref(), Some("2024-06-02"));
    let valid = store
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2023-01-01".into()),
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(valid[0].object, "Chicago");
    let current = store
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(current[0].object, "Boston");
    let end = store
        .triple_end(&bank, "Alice", "lives_in", None, "2025-01-01", "2025-01-01")
        .unwrap();
    assert!(!end.closed_ids.is_empty(), "{:?}", end.closed_ids);
    let noop = store
        .triple_end(&bank, "Alice", "lives_in", None, "2025-01-01", "2025-01-01")
        .unwrap();
    assert!(noop.already_ended);
}

#[test]
fn postgres_triple_context_parity() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-tctx");
    let ic = unique("ic");
    let ec = unique("ec");
    let ino = unique("ino");
    let eno = unique("eno");
    let mut with_ctx = carrier(&ic, &bank, "Boston", "2024-01-01", "2024-01-02");
    with_ctx.context = Some("pg carrier context".to_owned());
    store
        .commit_triple_add(
            &with_ctx,
            &edge(&ec, &bank, &ic, "Boston", "2024-01-01", "2024-01-02"),
            true,
            "agent",
        )
        .unwrap();
    store
        .commit_triple_add(
            &carrier(&ino, &bank, "NYC", "2024-02-01", "2024-02-02"),
            &edge(&eno, &bank, &ino, "NYC", "2024-02-01", "2024-02-02"),
            true,
            "agent",
        )
        .unwrap();
    let got = store.get_triple(&bank, &ec).unwrap().unwrap();
    assert_eq!(got.context.as_deref(), Some("pg carrier context"));
    let plain = store.get_triple(&bank, &eno).unwrap().unwrap();
    assert!(plain.context.is_none());
    assert!(!serde_json::to_string(&plain).unwrap().contains("context"));
    let rows = store
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: Some("Boston".into()),
            // as_of pinned inside the Boston interval: NYC supersedes at 2024-02-01.
            as_of: Some("2024-01-15".into()),
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].context.as_deref(), Some("pg carrier context"));
    let listed = store.list_triples(&bank).unwrap();
    let listed_ctx = listed
        .iter()
        .find(|t| t.id == ec)
        .and_then(|t| t.context.clone());
    assert_eq!(listed_ctx.as_deref(), Some("pg carrier context"));
}
