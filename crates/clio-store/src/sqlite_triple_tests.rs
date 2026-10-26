// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for core SQLite bi-temporal triple persistence tests.
//!
//! ## Owns
//! - Assert/supersede, as-of divergence, end/invalidate, and query-semantics cases
//!
//! ## Does not own
//! - Concurrency and guard edge cases (see `triple_edge_tests`)
//! - Postgres parity (see `pg_triple_tests`)
//!
//! ## Boundary
//!
//! Changes to core triple write/query behavior should normally update tests here.
//! Changes to Postgres wiring should not require changes here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SourceType, TimeAxis,
    TripleQuery, TripleRecord, UpdateRule, ensure_discrete_update_rule,
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
fn t01_triple_add_persists_open_ends() {
    let s = store();
    let bank = unique("b");
    let item = carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston");
    let e = edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02");
    let out = s.commit_triple_add(&item, &e, true, "agent").unwrap();
    assert_eq!(out.new_edge_id, "e1");
    assert!(out.prior_edge_ids.is_empty(), "{:?}", out.prior_edge_ids);
    let got = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert!(got.is_open());
    assert_eq!(got.object, "Boston");
}

#[test]
fn t02_supersede_closes_both_axes_not_delete() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2020-01-01", "2024-01-01", "Chicago"),
        &edge("e1", &bank, "i1", "Chicago", "2020-01-01", "2024-01-01"),
        true,
        "agent",
    )
    .unwrap();
    let out = s
        .commit_triple_add(
            &carrier("i2", &bank, "2024-06-01", "2024-06-02", "Boston"),
            &edge("e2", &bank, "i2", "Boston", "2024-06-01", "2024-06-02"),
            true,
            "agent",
        )
        .unwrap();
    assert_eq!(out.prior_edge_ids, vec!["e1".to_owned()]);
    let prior = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(prior.valid_until.as_deref(), Some("2024-06-01"));
    assert_eq!(prior.tx_until.as_deref(), Some("2024-06-02"));
    let cur = s.get_triple(&bank, "e2").unwrap().unwrap();
    assert!(cur.is_open());
    assert_eq!(cur.object, "Boston");
}

#[test]
fn t03_t04_t04b_as_of_axes_diverge_on_late_correction() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2020-01-01", "2024-01-01", "Chicago"),
        &edge("e1", &bank, "i1", "Chicago", "2020-01-01", "2024-01-01"),
        true,
        "agent",
    )
    .unwrap();
    s.commit_triple_add(
        &carrier("i2", &bank, "2020-01-01", "2024-09-01", "Boston"),
        &edge("e2", &bank, "i2", "Boston", "2020-01-01", "2024-09-01"),
        true,
        "agent",
    )
    .unwrap();
    let valid_now = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2024-10-01".into()),
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(valid_now.len(), 1);
    assert_eq!(valid_now[0].object, "Boston");
    let believed_then = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2024-06-01".into()),
            time_axis: TimeAxis::Transaction,
        })
        .unwrap();
    assert_eq!(believed_then.len(), 1);
    assert_eq!(believed_then[0].object, "Chicago");
    let believed_after = s
        .query_triples(&TripleQuery {
            bank_id: bank,
            subject: None,
            predicate: None,
            object: None,
            as_of: Some("2024-09-02".into()),
            time_axis: TimeAxis::Transaction,
        })
        .unwrap();
    assert_eq!(believed_after[0].object, "Boston");
}

#[test]
fn t05_triple_end_retains_history() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston"),
        &edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    let end = s
        .triple_end(&bank, "Alice", "lives_in", None, "2025-01-01", "2025-01-01")
        .unwrap();
    assert_eq!(end.closed_ids, vec!["e1".to_owned()]);
    assert!(!end.already_ended);
    let current = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert!(current.is_empty(), "{current:?}");
    let hist = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(hist.valid_until.as_deref(), Some("2025-01-01"));
    let noop = s
        .triple_end(&bank, "Alice", "lives_in", None, "2025-01-01", "2025-01-01")
        .unwrap();
    assert!(noop.already_ended);
}

#[test]
fn t06_invalidate_item_not_spo() {
    let s = store();
    let bank = unique("b");
    let mut item = carrier("item-a", &bank, "2024-01-01", "2024-01-02", "x");
    item.episodic_type = Some(EpisodicType::Gist);
    s.create_memory_item(&item, "agent").unwrap();
    let out = s
        .invalidate_item(
            &bank,
            "item-a",
            "2024-02-01",
            "2024-02-01",
            Some("item-b"),
            "agent",
        )
        .unwrap();
    assert_eq!(out.closed_ids, vec!["item-a".to_owned()]);
    assert_eq!(out.replacement_id.as_deref(), Some("item-b"));
    let got = s.get_memory_item(&bank, "item-a").unwrap().unwrap();
    assert_eq!(got.valid_until.as_deref(), Some("2024-02-01"));
    assert_eq!(got.tx_until.as_deref(), Some("2024-02-01"));
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston"),
        &edge("edge-x", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    let err = s
        .invalidate_item(&bank, "edge-x", "2024-02-01", "2024-02-01", None, "agent")
        .unwrap_err();
    assert!(err.message.contains("triple_end"));
}

#[test]
fn t07_continuous_rejected() {
    let err = ensure_discrete_update_rule(Some(UpdateRule::Continuous)).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    let s = store();
    let bank = unique("b");
    s.test_execute(&format!(
        "INSERT INTO attribute_schemas (bank_id, attr_key, attr_class, created_at)
         VALUES ('{bank}', 'lives_in', 'continuous', 't0')"
    ))
    .unwrap();
    let err = s
        .commit_triple_add(
            &carrier("i1", &bank, "2024-01-01", "2024-01-02", "0.82"),
            &edge("e1", &bank, "i1", "0.82", "2024-01-01", "2024-01-02"),
            true,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
}

#[test]
fn t09_belief_requires_source_type() {
    let s = store();
    let bank = unique("b");
    let mut item = carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston");
    item.epistemic_kind = EpistemicKind::Belief;
    let mut e = edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02");
    e.epistemic_kind = EpistemicKind::Belief;
    assert!(s.commit_triple_add(&item, &e, true, "agent").is_err());
    item.source_type = Some(SourceType::UserStated);
    e.source_type = Some(SourceType::UserStated);
    s.commit_triple_add(&item, &e, true, "agent").unwrap();
}

#[test]
fn query_object_filter_and_current_semantics() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston"),
        &edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    // Object constraint is pushed into the SQL WHERE and still matches.
    let by_object = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: Some("Boston".into()),
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(by_object.len(), 1);
    assert_eq!(by_object[0].id, "e1");
    // A future-dated fact is not returned as current.
    let mut future = edge("e2", &bank, "i2", "Mars", "9999-01-01", "2024-01-02");
    future.predicate = "visits".into();
    s.commit_triple_add(
        &carrier("i2", &bank, "9999-01-01", "2024-01-02", "Mars"),
        &future,
        true,
        "agent",
    )
    .unwrap();
    let current = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("visits".into()),
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert!(
        current.is_empty(),
        "future fact must not be current: {current:?}"
    );
    let as_to_future = s
        .query_triples(&TripleQuery {
            bank_id: bank,
            subject: Some("Alice".into()),
            predicate: Some("visits".into()),
            object: None,
            as_of: Some("9999-06-01".into()),
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(as_to_future.len(), 1);
    assert_eq!(as_to_future[0].object, "Mars");
}

#[test]
fn t11_triple_context_surfaces_from_sealed_carrier() {
    let s = store();
    let bank = unique("b");
    let mut item = carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston");
    item.context = Some("Q1 review notes".into());
    s.commit_triple_add(
        &item,
        &edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    let mut item2 = carrier("i2", &bank, "2024-02-01", "2024-02-02", "NYC");
    item2.context = Some("second note".into());
    let mut e2 = edge("e2", &bank, "i2", "NYC", "2024-02-01", "2024-02-02");
    e2.predicate = "visits".into();
    s.commit_triple_add(&item2, &e2, true, "agent").unwrap();
    let got = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(got.context.as_deref(), Some("Q1 review notes"));
    let listed = s.list_triples(&bank).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].context.as_deref(), Some("Q1 review notes"));
    let rows = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(rows.len(), 2);
    // Stable id ordering on a multi-row result.
    assert_eq!((rows[0].id.as_str(), rows[1].id.as_str()), ("e1", "e2"));
    assert_eq!(rows[0].context.as_deref(), Some("Q1 review notes"));
    assert_eq!(rows[1].context.as_deref(), Some("second note"));
}

#[test]
fn t12_triple_without_context_omits_wire_key() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston"),
        &edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    let got = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert!(got.context.is_none());
    let wire = serde_json::to_string(&got).unwrap();
    assert!(!wire.contains("context"), "{wire}");
}

#[test]
fn t13_supersede_same_second_keeps_prior_in_history() {
    let s = store();
    let bank = unique("b");
    let t = "2024-05-01T00:00:00Z";
    let add = |item: &str, id: &str, object: &str| {
        s.commit_triple_add(
            &carrier(item, &bank, "2024-01-01", t, object),
            &edge(id, &bank, item, object, "2024-01-01", t),
            true,
            "agent",
        )
        .unwrap()
    };
    assert!(add("i1", "e1", "Chicago").prior_edge_ids.is_empty());
    let closed = add("i2", "e2", "Boston").prior_edge_ids;
    assert_eq!(closed, vec!["e1".to_owned()]);
    let prior = s.get_triple(&bank, "e1").unwrap().unwrap();
    let until = prior.tx_until.clone().expect("prior row is closed");
    assert_eq!(until, "2024-05-01T00:00:01Z", "close lands one second on");
    assert!(until > prior.tx_from, "{until} vs {}", prior.tx_from);
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
