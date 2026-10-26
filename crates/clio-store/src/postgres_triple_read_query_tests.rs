// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres triple query, list, and item
//! invalidation cases owned by `postgres_triple_read`.
//!
//! ## Owns
//! - `query_triples` on both axes, inside and outside the interval, with the
//!   subject / predicate / object filters
//! - `get_triple`, `list_triples`, and `invalidate_item`, including the guards
//!   and the already-ended path
//!
//! ## Does not own
//! - `triple_end` cases (see `postgres_triple_read_tests`)
//! - Atomic supersede writes and their close stamps (see `postgres_triple_tests`)
//!
//! ## Boundary
//!
//! Changes to the Postgres triple query, list, or invalidate paths should
//! normally update tests here.

use crate::PgTestGuard;
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

const OPEN_FROM: &str = "2024-01-01T00:00:00Z";
const OPEN_TX: &str = "2024-02-01T00:00:00Z";

fn open_pg() -> Option<PostgresStore> {
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    match PostgresStore::open(&url, kms) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres triple read tests", &err);
            None
        }
    }
}

fn setup() -> Option<(PgTestGuard, PostgresStore)> {
    let guard = lock_pg_test();
    let store = open_pg()?;
    Some((guard, store))
}

fn key(bank: &str, id: &str) -> String {
    format!("{bank}-{id}")
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
    bank: &str,
    subject: &str,
    id: &str,
    object: &str,
    valid_from: &str,
    tx_from: &str,
) -> TripleRecord {
    TripleRecord {
        id: format!("{bank}-{id}"),
        bank_id: bank.into(),
        item_id: format!("item-{bank}-{id}"),
        subject: subject.into(),
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

fn add_open(store: &PostgresStore, bank: &str, subject: &str, id: &str, object: &str) {
    store
        .commit_triple_add(
            &carrier(
                &format!("item-{bank}-{id}"),
                bank,
                object,
                OPEN_FROM,
                OPEN_TX,
            ),
            &edge(bank, subject, id, object, OPEN_FROM, OPEN_TX),
            true,
            "agent",
        )
        .expect("triple add");
}

fn query(
    bank: &str,
    subject: Option<&str>,
    predicate: Option<&str>,
    object: Option<&str>,
    as_of: Option<&str>,
    time_axis: TimeAxis,
) -> TripleQuery {
    TripleQuery {
        bank_id: bank.into(),
        subject: subject.map(str::to_owned),
        predicate: predicate.map(str::to_owned),
        object: object.map(str::to_owned),
        as_of: as_of.map(str::to_owned),
        time_axis,
    }
}

#[test]
fn query_triples_transaction_axis_brackets_the_interval() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-qt");
    let subject = unique("s-qt");
    add_open(&s, &bank, &subject, "e1", "Boston");
    s.triple_end(
        &bank,
        &subject,
        "lives_in",
        None,
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let inside = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some("2025-01-01T00:00:00Z"),
            TimeAxis::Transaction,
        ))
        .unwrap();
    assert_eq!(inside.len(), 1);
    let after = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some("2027-01-01T00:00:00Z"),
            TimeAxis::Transaction,
        ))
        .unwrap();
    assert!(after.is_empty(), "{after:?}");
}

#[test]
fn query_triples_applies_pattern_filters() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-qp");
    let subject = unique("s-qp");
    add_open(&s, &bank, &subject, "e1", "Boston");
    let at = Some("2024-06-01T00:00:00Z");
    let exact = s
        .query_triples(&query(
            &bank,
            Some(&subject),
            Some("lives_in"),
            None,
            at,
            TimeAxis::Valid,
        ))
        .unwrap();
    assert_eq!(exact.len(), 1);
    let other_subject = s
        .query_triples(&query(
            &bank,
            Some("somebody-else"),
            None,
            None,
            at,
            TimeAxis::Valid,
        ))
        .unwrap();
    assert!(other_subject.is_empty(), "{other_subject:?}");
    let other_predicate = s
        .query_triples(&query(
            &bank,
            None,
            Some("visits"),
            None,
            at,
            TimeAxis::Valid,
        ))
        .unwrap();
    assert!(other_predicate.is_empty(), "{other_predicate:?}");
    let by_object = s
        .query_triples(&query(
            &bank,
            None,
            None,
            Some("Boston"),
            at,
            TimeAxis::Valid,
        ))
        .unwrap();
    assert_eq!(by_object.len(), 1);
    assert_eq!(by_object[0].id, key(&bank, "e1"));
    let other_object = s
        .query_triples(&query(
            &bank,
            None,
            None,
            Some("Chicago"),
            at,
            TimeAxis::Valid,
        ))
        .unwrap();
    assert!(other_object.is_empty(), "{other_object:?}");
}

#[test]
fn query_triples_requires_a_bank() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let err = s
        .query_triples(&query("", None, None, None, None, TimeAxis::Valid))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
}

#[test]
fn get_triple_returns_none_for_unknown_edge() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-miss");
    assert!(s.get_triple(&bank, "edge-absent").unwrap().is_none());
}

#[test]
fn get_triple_surfaces_carrier_context() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-ctx");
    let subject = unique("s-ctx");
    let mut item = carrier(
        &format!("item-{bank}-e1"),
        &bank,
        "Boston",
        OPEN_FROM,
        OPEN_TX,
    );
    item.context = Some("Q1 review notes".into());
    s.commit_triple_add(
        &item,
        &edge(&bank, &subject, "e1", "Boston", OPEN_FROM, OPEN_TX),
        true,
        "agent",
    )
    .unwrap();
    let plain_subject = unique("s-ctx-plain");
    add_open(&s, &bank, &plain_subject, "e2", "NYC");
    let with_context = s.get_triple(&bank, &key(&bank, "e1")).unwrap().unwrap();
    assert_eq!(with_context.context.as_deref(), Some("Q1 review notes"));
    let plain = s.get_triple(&bank, &key(&bank, "e2")).unwrap().unwrap();
    assert!(plain.context.is_none(), "{:?}", plain.context);
}

#[test]
fn list_triples_requires_bank_and_maps_rows() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-list");
    let subject = unique("s-list");
    let mut item = carrier(
        &format!("item-{bank}-e1"),
        &bank,
        "Boston",
        OPEN_FROM,
        OPEN_TX,
    );
    item.context = Some("review note".into());
    s.commit_triple_add(
        &item,
        &edge(&bank, &subject, "e1", "Boston", OPEN_FROM, OPEN_TX),
        true,
        "agent",
    )
    .unwrap();
    let plain_subject = unique("s-list-plain");
    add_open(&s, &bank, &plain_subject, "e2", "NYC");
    let err = s.list_triples("").unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
    let listed = s.list_triples(&bank).unwrap();
    let ids: Vec<String> = listed.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids, vec![key(&bank, "e1"), key(&bank, "e2")]);
    assert_eq!(listed[0].context.as_deref(), Some("review note"));
    assert!(listed[1].context.is_none(), "{:?}", listed[1].context);
}

#[test]
fn invalidate_item_rejects_blank_arguments() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-inv");
    let no_bank = s
        .invalidate_item(
            "",
            &format!("item-{bank}-a"),
            "2025-03-01T00:00:00Z",
            "2025-03-01T00:00:00Z",
            None,
            "agent",
        )
        .unwrap_err();
    assert_eq!(no_bank.code, ErrorCode::InvalidArgument);
    let no_item = s
        .invalidate_item(
            &bank,
            "",
            "2025-03-01T00:00:00Z",
            "2025-03-01T00:00:00Z",
            None,
            "agent",
        )
        .unwrap_err();
    assert_eq!(no_item.code, ErrorCode::InvalidArgument);
    let no_actor = s
        .invalidate_item(
            &bank,
            &format!("item-{bank}-a"),
            "2025-03-01T00:00:00Z",
            "2025-03-01T00:00:00Z",
            None,
            "",
        )
        .unwrap_err();
    assert_eq!(no_actor.code, ErrorCode::InvalidArgument);
}

#[test]
fn invalidate_item_refuses_spo_edge_ids() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-inv-spo");
    let subject = unique("s-inv-spo");
    add_open(&s, &bank, &subject, "edge-x", "Boston");
    let err = s
        .invalidate_item(
            &bank,
            &key(&bank, "edge-x"),
            "2025-03-01T00:00:00Z",
            "2025-03-01T00:00:00Z",
            None,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("triple_end"), "{}", err.message);
}

#[test]
fn invalidate_item_closes_both_axes_and_is_idempotent() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-inv-ok");
    let mut item = carrier(&format!("item-{bank}-a"), &bank, "x", OPEN_FROM, OPEN_TX);
    item.episodic_type = Some(EpisodicType::Gist);
    s.create_memory_item(&item, "agent").unwrap();
    let out = s
        .invalidate_item(
            &bank,
            &format!("item-{bank}-a"),
            "2025-03-01T00:00:00Z",
            "2025-03-02T00:00:00Z",
            Some("item-b"),
            "agent",
        )
        .unwrap();
    assert_eq!(out.closed_ids, vec![format!("item-{bank}-a")]);
    assert!(!out.already_ended);
    assert_eq!(out.replacement_id.as_deref(), Some("item-b"));
    let got = s
        .get_memory_item(&bank, &format!("item-{bank}-a"))
        .unwrap()
        .unwrap();
    assert_eq!(got.valid_until.as_deref(), Some("2025-03-01T00:00:00Z"));
    assert_eq!(got.tx_until.as_deref(), Some("2025-03-02T00:00:00Z"));
    let again = s
        .invalidate_item(
            &bank,
            &format!("item-{bank}-a"),
            "2025-04-01T00:00:00Z",
            "2025-04-01T00:00:00Z",
            None,
            "agent",
        )
        .unwrap();
    assert!(again.already_ended);
    assert_eq!(again.closed_ids, vec![format!("item-{bank}-a")]);
}
