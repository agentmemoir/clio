// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite triple concurrency and guard edge-case tests.
//!
//! ## Owns
//! - Concurrent supersede, cross-bank isolation, and rejection-path cases
//! - Bounded-valid / transaction-open supersession cases
//!
//! ## Does not own
//! - Core assert/supersede/query happy paths (see `triple_tests`)
//!
//! ## Boundary
//!
//! Changes to triple guard/error behavior should normally update tests here.
//! Changes to Postgres wiring should not require changes here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, TimeAxis, TripleQuery,
    TripleRecord,
};
use serde_json::json;
use std::sync::Arc;
use std::thread;

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
        context: None,
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
        created_at: tx_from.into(),
        updated_at: None,
    }
}

#[test]
fn supersede_closes_tx_open_bounded_valid_edge() {
    let s = store();
    let bank = unique("b");
    let mut bounded = edge("e1", &bank, "i1", "Chicago", "2020-01-01", "2024-01-01");
    bounded.valid_until = Some("2021-01-01".into());
    s.commit_triple_add(
        &carrier("i1", &bank, "2020-01-01", "2024-01-01", "Chicago"),
        &bounded,
        true,
        "agent",
    )
    .unwrap();
    let before = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(before.valid_until.as_deref(), Some("2021-01-01"));
    assert!(before.tx_until.is_none());
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
    assert_eq!(prior.tx_until.as_deref(), Some("2024-06-02"));
    assert_eq!(
        prior.valid_until.as_deref(),
        Some("2021-01-01"),
        "existing valid end must not be extended"
    );
}

#[test]
fn t11_concurrent_supersede_one_open() {
    let s = Arc::new(store());
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i0", &bank, "2020-01-01", "2024-01-01", "Chicago"),
        &edge("e0", &bank, "i0", "Chicago", "2020-01-01", "2024-01-01"),
        true,
        "agent",
    )
    .unwrap();
    let bank_a = bank.clone();
    let bank_b = bank.clone();
    let sa = Arc::clone(&s);
    let sb = Arc::clone(&s);
    let t1 = thread::spawn(move || {
        sa.commit_triple_add(
            &carrier("ia", &bank_a, "2024-06-01", "2024-06-02", "Boston"),
            &edge("ea", &bank_a, "ia", "Boston", "2024-06-01", "2024-06-02"),
            true,
            "agent",
        )
    });
    let t2 = thread::spawn(move || {
        sb.commit_triple_add(
            &carrier("ib", &bank_b, "2024-06-01", "2024-06-03", "NYC"),
            &edge("eb", &bank_b, "ib", "NYC", "2024-06-01", "2024-06-03"),
            true,
            "agent",
        )
    });
    t1.join().unwrap().unwrap();
    t2.join().unwrap().unwrap();
    let open = s
        .query_triples(&TripleQuery {
            bank_id: bank,
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(open.len(), 1);
}

#[test]
fn t12_cross_bank_closed() {
    let s = store();
    let bank_a = unique("a");
    let bank_b = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank_a, "2024-01-01", "2024-01-02", "Boston"),
        &edge("e1", &bank_a, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    assert!(s.get_triple(&bank_b, "e1").unwrap().is_none());
    let q = s
        .query_triples(&TripleQuery {
            bank_id: bank_b,
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert!(q.is_empty(), "{q:?}");
}

#[test]
fn supersede_false_rejects_open_coexistence() {
    let s = store();
    let bank = unique("b");
    s.commit_triple_add(
        &carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston"),
        &edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02"),
        true,
        "agent",
    )
    .unwrap();
    let err = s
        .commit_triple_add(
            &carrier("i2", &bank, "2024-02-01", "2024-02-02", "NYC"),
            &edge("e2", &bank, "i2", "NYC", "2024-02-01", "2024-02-02"),
            false,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn commit_rejects_bad_carrier_and_missing_snapshot_ref() {
    let s = store();
    let bank = unique("b");
    let mut item = carrier("i1", &bank, "2024-01-01", "2024-01-02", "Boston");
    item.episodic_type = Some(EpisodicType::Gist);
    let e = edge("e1", &bank, "i1", "Boston", "2024-01-01", "2024-01-02");
    assert!(s.commit_triple_add(&item, &e, true, "agent").is_err());
    item = carrier("i2", &bank, "2024-01-01", "2024-01-02", "Boston");
    let mut e2 = edge("e2", &bank, "other", "Boston", "2024-01-01", "2024-01-02");
    assert!(s.commit_triple_add(&item, &e2, true, "agent").is_err());
    e2.item_id = "i2".into();
    e2.snapshot_ref = Some("missing-snap".into());
    let err = s.commit_triple_add(&item, &e2, true, "agent").unwrap_err();
    assert!(err.message.contains("snapshot_ref"));
    assert!(s.commit_triple_add(&item, &e2, true, "").is_err());
    // Hit the snapshot_ref row-mapper closure (found path).
    let snap_id = unique("snap");
    let mut snap = carrier(&snap_id, &bank, "snap", "2024-01-01", "2024-01-02");
    snap.episodic_type = Some(EpisodicType::Gist);
    s.create_memory_item(&snap, "agent").unwrap();
    let iid = unique("i3");
    let eid = unique("e3");
    let mut e3 = edge(&eid, &bank, &iid, "Boston", "2024-01-01", "2024-01-02");
    e3.snapshot_ref = Some(snap_id);
    s.commit_triple_add(
        &carrier(&iid, &bank, "Boston", "2024-01-01", "2024-01-02"),
        &e3,
        true,
        "agent",
    )
    .unwrap();
    // Force items-insert Err → map_err(|e| e.to_string()) runs.
    let err = s
        .commit_triple_add(
            &carrier(&iid, &bank, "Dup", "2024-05-01", "2024-05-02"),
            &edge(
                &unique("edup"),
                &bank,
                &iid,
                "Dup",
                "2024-05-01",
                "2024-05-02",
            ),
            true,
            "agent",
        )
        .unwrap_err();
    assert!(
        err.message.contains("duplicate") || err.message.contains("items insert"),
        "{err}"
    );
}

#[test]
fn query_end_invalidate_edge_cases() {
    let s = store();
    let bank = unique("b");
    assert!(
        s.query_triples(&TripleQuery {
            bank_id: String::new(),
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .is_err()
    );
    assert!(s.triple_end("", "a", "b", None, "t", "t").is_err());
    assert!(s.triple_end(&bank, "a", "b", None, "", "t").is_err());
    let noop = s
        .triple_end(&bank, "nobody", "lives_in", None, "t1", "t1")
        .unwrap();
    assert!(noop.already_ended);
    assert!(
        s.invalidate_item(&bank, "no-item", "t1", "t1", None, "agent")
            .is_err()
    );
    let mut item = carrier("gist1", &bank, "t0", "t0", "x");
    item.episodic_type = Some(EpisodicType::Gist);
    s.create_memory_item(&item, "agent").unwrap();
    s.invalidate_item(&bank, "gist1", "t1", "t1", None, "agent")
        .unwrap();
    let again = s
        .invalidate_item(&bank, "gist1", "t2", "t2", None, "agent")
        .unwrap();
    assert!(again.already_ended);
    assert!(
        s.invalidate_item("", "gist1", "t", "t", None, "agent")
            .is_err()
    );
}
