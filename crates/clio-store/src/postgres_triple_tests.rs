// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity tests of bi-temporal triples.
//!
//! ## Owns
//! - As-of parity, invalidate/end guards, continuous rejection, and query pushdown cases
//!
//! ## Does not own
//! - SQLite triple behavior (see `triple_tests` / `triple_edge_tests`)
//! - Pool open/migrate (see `postgres`)
//!
//! ## Boundary
//!
//! Changes to Postgres triple SQL should normally update tests here.
//! Changes to SQLite-only behavior should not require changes here.

use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, TimeAxis, TripleQuery,
    TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

fn open_pg() -> Option<PostgresStore> {
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

fn carrier(id: &str, bank: &str, object: &str, valid_from: &str, tx_from: &str) -> MemoryItem {
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
fn postgres_invalidate_and_guards() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-inv");
    pg_invalidate_gist_and_edge_id(&store, &bank);
    pg_continuous_rejected(&store, &bank);
    let bank2 = unique("pg-co");
    pg_supersede_false_and_lookup_guards(&store, &bank2);
    pg_commit_guards_and_duplicate(&store, &bank2);
    pg_query_object_filter(&store, &bank2);
}

fn pg_invalidate_gist_and_edge_id(store: &PostgresStore, bank: &str) {
    let gist_id = unique("gist");
    let mut item = carrier(&gist_id, bank, "x", "t0", "t0");
    item.episodic_type = Some(EpisodicType::Gist);
    store.create_memory_item(&item, "agent").unwrap();
    store
        .invalidate_item(bank, &gist_id, "t1", "t1", Some("r1"), "agent")
        .unwrap();
    let again = store
        .invalidate_item(bank, &gist_id, "t2", "t2", None, "agent")
        .unwrap();
    assert!(again.already_ended);
    let i1 = unique("i1");
    let edge_x = unique("edge");
    store
        .commit_triple_add(
            &carrier(&i1, bank, "Boston", "2024-01-01", "2024-01-02"),
            &edge(&edge_x, bank, &i1, "Boston", "2024-01-01", "2024-01-02"),
            true,
            "agent",
        )
        .unwrap();
    assert!(
        store
            .invalidate_item(bank, &edge_x, "t", "t", None, "agent")
            .unwrap_err()
            .message
            .contains("triple_end")
    );
}

fn pg_continuous_rejected(store: &PostgresStore, bank: &str) {
    store
        .test_execute(&format!(
            "INSERT INTO attribute_schemas (bank_id, attr_key, attr_class, created_at)
             VALUES ('{bank}', 'lives_in', 'continuous', 't0')
             ON CONFLICT (bank_id, attr_key) DO NOTHING"
        ))
        .ok();
    let i3 = unique("i3");
    let e3 = unique("e3");
    let err = store
        .commit_triple_add(
            &carrier(&i3, bank, "0.5", "2024-02-01", "2024-02-02"),
            &edge(&e3, bank, &i3, "0.5", "2024-02-01", "2024-02-02"),
            true,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
}

fn pg_supersede_false_and_lookup_guards(store: &PostgresStore, bank2: &str) {
    let ia = unique("ia");
    let ib = unique("ib");
    let ea = unique("ea");
    let eb = unique("eb");
    store
        .commit_triple_add(
            &carrier(&ia, bank2, "Boston", "2024-01-01", "2024-01-02"),
            &edge(&ea, bank2, &ia, "Boston", "2024-01-01", "2024-01-02"),
            true,
            "agent",
        )
        .unwrap();
    let err = store
        .commit_triple_add(
            &carrier(&ib, bank2, "NYC", "2024-02-01", "2024-02-02"),
            &edge(&eb, bank2, &ib, "NYC", "2024-02-01", "2024-02-02"),
            false,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(store.get_triple(bank2, "missing").unwrap().is_none());
    assert!(store.triple_end("", "a", "b", None, "t", "t").is_err());
    assert_eq!(
        store
            .invalidate_item(bank2, "nope", "t", "t", None, "agent")
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
}

fn pg_commit_guards_and_duplicate(store: &PostgresStore, bank2: &str) {
    pg_commit_guard_errors(store, bank2);
    pg_snapshot_belief_and_duplicate(store, bank2);
}

fn pg_commit_guard_errors(store: &PostgresStore, bank2: &str) {
    let bad = unique("bad");
    let mut item = carrier(&bad, bank2, "x", "2024-01-01", "2024-01-02");
    item.episodic_type = Some(EpisodicType::Gist);
    assert!(
        store
            .commit_triple_add(
                &item,
                &edge(
                    &unique("ebad"),
                    bank2,
                    &bad,
                    "x",
                    "2024-01-01",
                    "2024-01-02"
                ),
                true,
                "agent",
            )
            .is_err()
    );
    let ok_id = unique("ok");
    let mut e = edge(
        &unique("eref"),
        bank2,
        &ok_id,
        "x",
        "2024-01-01",
        "2024-01-02",
    );
    e.snapshot_ref = Some("missing-ref".into());
    assert!(
        store
            .commit_triple_add(
                &carrier(&ok_id, bank2, "x", "2024-01-01", "2024-01-02"),
                &e,
                true,
                "agent",
            )
            .is_err()
    );
    assert!(
        store
            .commit_triple_add(
                &carrier(&unique("act"), bank2, "x", "2024-01-01", "2024-01-02"),
                &edge(
                    &unique("eact"),
                    bank2,
                    "mismatch",
                    "x",
                    "2024-01-01",
                    "2024-01-02"
                ),
                true,
                "agent",
            )
            .is_err()
    );
}

fn pg_snapshot_belief_and_duplicate(store: &PostgresStore, bank2: &str) {
    // Hit snapshot_ref found-path row mapper.
    let snap_id = unique("snap");
    let mut snap = carrier(&snap_id, bank2, "snap", "2024-01-01", "2024-01-02");
    snap.episodic_type = Some(EpisodicType::Gist);
    store.create_memory_item(&snap, "agent").unwrap();
    let iid = unique("iok");
    let eid = unique("eok");
    let mut eok = edge(&eid, bank2, &iid, "Paris", "2024-04-01", "2024-04-02");
    eok.snapshot_ref = Some(snap_id);
    store
        .commit_triple_add(
            &carrier(&iid, bank2, "Paris", "2024-04-01", "2024-04-02"),
            &eok,
            true,
            "agent",
        )
        .unwrap();
    // Belief source_type.map + duplicate insert map_err.
    let ib = unique("ibel");
    let eb = unique("ebel");
    let mut belief_item = carrier(&ib, bank2, "Maybe", "2024-05-01", "2024-05-02");
    belief_item.epistemic_kind = EpistemicKind::Belief;
    belief_item.source_type = Some(clio_types::SourceType::UserStated);
    let mut belief_edge = edge(&eb, bank2, &ib, "Maybe", "2024-05-01", "2024-05-02");
    belief_edge.epistemic_kind = EpistemicKind::Belief;
    belief_edge.source_type = Some(clio_types::SourceType::UserStated);
    store
        .commit_triple_add(&belief_item, &belief_edge, true, "agent")
        .unwrap();
    let err = store
        .commit_triple_add(
            &carrier(&iid, bank2, "Dup", "2024-06-01", "2024-06-02"),
            &edge(
                &unique("edup"),
                bank2,
                &iid,
                "Dup",
                "2024-06-01",
                "2024-06-02",
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

fn pg_query_object_filter(store: &PostgresStore, bank: &str) {
    let rows = store
        .query_triples(&TripleQuery {
            bank_id: bank.into(),
            subject: None,
            predicate: None,
            object: Some("Maybe".into()),
            as_of: None,
            time_axis: TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object, "Maybe");
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
