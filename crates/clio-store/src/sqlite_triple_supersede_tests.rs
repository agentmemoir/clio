// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite supersede case where the closing
//! stamp lands inside the row's own second: the prior row keeps a non-empty
//! valid interval and stays readable on the transaction axis, and the instant it
//! was closed at shows the closed row alongside its successor.
//!
//! ## Owns
//! - Same-second supersede history and the overlap that widening leaves at the
//!   closing instant
//!
//! ## Does not own
//! - The rest of the SQLite triple write and query surface (`sqlite_triple_tests`)
//! - The close-stamp rule itself (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what a
//! same-second supersede leaves readable should normally update tests here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ItemKind, MemoryItem, TimeAxis, TripleQuery, TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn carrier(id: &str, bank: &str, valid_from: &str, tx_from: &str, object: &str) -> MemoryItem {
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

fn edge(
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
fn t13_supersede_same_second_keeps_prior_in_history() {
    let s = store();
    let bank = unique("b");
    let t = "2024-05-01T00:00:00Z";
    let add = |item: &str, id: &str, object: &str, valid_from: &str| {
        s.commit_triple_add(
            &carrier(item, &bank, valid_from, t, object),
            &edge(id, &bank, item, object, valid_from, t),
            true,
            "agent",
        )
        .unwrap()
    };
    let opened = add("i1", "e1", "Chicago", "2024-01-01").prior_edge_ids;
    assert!(opened.is_empty(), "{opened:?}");
    let closed = add("i2", "e2", "Boston", "2024-06-01").prior_edge_ids;
    assert_eq!(closed, vec!["e1".to_owned()]);
    let prior = s.get_triple(&bank, "e1").unwrap().unwrap();
    let until = prior.tx_until.clone().expect("prior row is closed");
    assert_eq!(until, "2024-05-01T00:00:01Z", "close lands one second on");
    assert!(until > prior.tx_from, "{until} vs {}", prior.tx_from);
    let valid_until = prior
        .valid_until
        .clone()
        .expect("prior row is closed on the valid axis too");
    assert!(
        valid_until > prior.valid_from,
        "valid interval [{}, {valid_until}) is empty",
        prior.valid_from
    );
    let believed = |as_of: &str| {
        s.query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: Some("Chicago".into()),
            as_of: Some(as_of.into()),
            time_axis: TimeAxis::Transaction,
        })
        .unwrap()
    };
    assert_eq!(believed(t).len(), 1, "visible at {t}");
    assert!(believed(&until).is_empty(), "prior belief gone at {until}");
}

#[test]
fn t14_supersede_same_second_shows_both_rows_at_the_closing_instant() {
    let s = store();
    let bank = unique("b");
    let t = "2024-05-01T00:00:00Z";
    let add = |item: &str, id: &str, object: &str, valid_from: &str| {
        s.commit_triple_add(
            &carrier(item, &bank, valid_from, t, object),
            &edge(id, &bank, item, object, valid_from, t),
            true,
            "agent",
        )
        .unwrap()
    };
    add("i1", "e1", "Chicago", "2024-01-01");
    add("i2", "e2", "Boston", "2024-06-01");
    let rows = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some(t.into()),
            time_axis: TimeAxis::Transaction,
        })
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["e1", "e2"],
        "the row closed in its own second stays readable for that second alongside its successor"
    );
}
