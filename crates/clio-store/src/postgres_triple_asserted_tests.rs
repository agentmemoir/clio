// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres ever-asserted lookup: a subject, predicate, and
//! optional object are found whether the triple is open or ended, and not found when
//! the bank never asserted them.
//!
//! ## Owns
//! - Assertions over Postgres `triple_asserted_ever` for absent, open, ended, and wrong-object triples
//!
//! ## Does not own
//! - The write layer's refusal of an unknown triple (see `clio-write`)
//! - The SQLite mirror (see `sqlite_triple_asserted_tests`)
//!
//! ## Boundary
//!
//! Changes to the lookup's answer should normally require changes here. Changes to
//! triple closing should not.
//!
//! Keep this module focused on the ever-asserted lookup.

use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use crate::triple_assertion::TripleAssertion;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem, TripleRecord};
use serde_json::json;
use std::sync::Arc;

fn open_pg() -> Option<PostgresStore> {
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    match PostgresStore::open(&url, kms) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres ever-asserted tests", &err);
            None
        }
    }
}

fn carrier(id: &str, bank: &str, object: &str) -> MemoryItem {
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
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-02".into(),
        tx_until: None,
        created_at: "2024-01-02".into(),
        updated_at: "2024-01-02".into(),
        context: None,
    }
}

fn edge(id: &str, bank: &str, item_id: &str, object: &str) -> TripleRecord {
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
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-02".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2024-01-02".into(),
        updated_at: None,
    }
}

/// Assert that Alice lives in Boston in a fresh bank. The ids are unique per call,
/// because the Postgres test database is shared. Returns the bank id.
fn asserted_boston(store: &PostgresStore) -> String {
    let bank = unique("b");
    let item = unique("i");
    let edge_id = unique("e");
    store
        .commit_triple_add(
            &carrier(&item, &bank, "Boston"),
            &edge(&edge_id, &bank, &item, "Boston"),
            true,
            "agent",
        )
        .expect("triple add");
    bank
}

#[test]
fn a_triple_the_bank_never_asserted_is_not_found_by_the_lookup() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("b");
    assert!(
        !store
            .triple_asserted_ever(&bank, "nobody", "lives_in", None)
            .expect("lookup")
    );
}

#[test]
fn an_open_triple_is_found_by_the_lookup() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = asserted_boston(&store);
    assert!(
        store
            .triple_asserted_ever(&bank, "Alice", "lives_in", None)
            .expect("lookup")
    );
}

#[test]
fn an_ended_triple_is_still_found_by_the_lookup() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = asserted_boston(&store);
    store
        .triple_end(&bank, "Alice", "lives_in", None, "2025-01-01", "2025-01-01")
        .expect("end");
    assert!(
        store
            .triple_asserted_ever(&bank, "Alice", "lives_in", None)
            .expect("lookup")
    );
}

#[test]
fn the_lookup_matches_the_object_when_one_is_given() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = asserted_boston(&store);
    assert!(
        store
            .triple_asserted_ever(&bank, "Alice", "lives_in", Some("Boston"))
            .expect("lookup")
    );
    assert!(
        !store
            .triple_asserted_ever(&bank, "Alice", "lives_in", Some("Paris"))
            .expect("lookup")
    );
}
