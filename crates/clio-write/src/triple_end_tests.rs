// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the write layer's triple end: a triple the bank never asserted
//! is refused as not found, an asserted triple closes, and a repeated end is a no-op.
//!
//! ## Owns
//! - Assertions over `triple_end` for absent, open, ended, and wrong-object triples
//!
//! ## Does not own
//! - The store's ever-asserted lookup (see `clio-store`)
//! - The core triple add and supersede paths (see `triple_tests`)
//!
//! ## Boundary
//!
//! Changes to the not-found rule should normally require changes here. Changes to
//! triple closing mechanics should not.
//!
//! Keep this module focused on the triple end's not-found rule.

use crate::triple::{TripleVecSink, triple_end};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, Bank, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
    TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn carrier(bank: &str) -> MemoryItem {
    MemoryItem {
        id: "i1".into(),
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
        snapshot: Some(json!({"object": "Boston"})),
        gist: Some("lives_in Boston".into()),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-02".into(),
        tx_until: None,
        created_at: "2024-01-02".into(),
        updated_at: "2024-01-02".into(),
        context: None,
    }
}

fn edge(bank: &str) -> TripleRecord {
    TripleRecord {
        id: "e1".into(),
        bank_id: bank.into(),
        item_id: "i1".into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
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

/// Assert that Alice lives in Boston, in a fresh bank.
fn asserted_boston(s: &SqliteStore, bank: &str) {
    s.commit_triple_add(&carrier(bank), &edge(bank), true, "agent")
        .expect("triple add");
}

fn end(
    s: &SqliteStore,
    sink: &TripleVecSink,
    object: Option<&str>,
) -> Result<clio_types::TripleEndOutcome, clio_types::AmError> {
    let ctx = OpContext::new(Bank::new("bank-end"), Actor::Agent);
    triple_end(
        s,
        &ctx,
        "Alice",
        "lives_in",
        object,
        "2025-01-01",
        "2025-01-01",
        None,
        sink,
    )
}

#[test]
fn an_end_of_a_triple_the_bank_never_asserted_is_not_found() {
    let s = store();
    let sink = TripleVecSink::default();
    let err = end(&s, &sink, None).expect_err("a never-asserted triple is not a no-op");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("Alice lives_in"), "{}", err.message);
}

#[test]
fn an_end_of_an_asserted_triple_closes_it() {
    let s = store();
    asserted_boston(&s, "bank-end");
    let sink = TripleVecSink::default();
    let out = end(&s, &sink, None).expect("end");
    assert_eq!(out.closed_ids, vec!["e1".to_owned()]);
    assert!(!out.already_ended);
}

#[test]
fn a_repeated_end_of_an_ended_triple_is_a_noop() {
    let s = store();
    asserted_boston(&s, "bank-end");
    let sink = TripleVecSink::default();
    end(&s, &sink, None).expect("first end");
    let again = end(&s, &sink, None).expect("an ended triple is not an error");
    assert!(again.already_ended);
    assert_eq!(again.closed_ids, Vec::<String>::new());
}

#[test]
fn an_end_naming_an_object_never_asserted_is_not_found_and_changes_nothing() {
    let s = store();
    asserted_boston(&s, "bank-end");
    let sink = TripleVecSink::default();
    let err = end(&s, &sink, Some("Paris")).expect_err("Paris was never asserted");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("Paris"), "{}", err.message);
    let still_open = s.get_triple("bank-end", "e1").expect("read").expect("row");
    assert!(
        still_open.is_open(),
        "a refused end must not close the triple"
    );
    assert!(sink.events().is_empty(), "a refused end emits no event");
}

#[test]
fn an_end_naming_the_asserted_object_closes_it() {
    let s = store();
    asserted_boston(&s, "bank-end");
    let sink = TripleVecSink::default();
    let out = end(&s, &sink, Some("Boston")).expect("end");
    assert_eq!(out.closed_ids, vec!["e1".to_owned()]);
}
