// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite triple end cases owned by
//! `sqlite_triple_read`.
//!
//! ## Owns
//! - `triple_end` closing, same-stamp, already-ended, and not-found cases
//! - The object filter and the argument guards on that path
//!
//! ## Does not own
//! - Triple query, list, and item invalidation cases (see
//!   `sqlite_triple_read_query_tests`)
//! - Atomic supersede writes and their close stamps (see `sqlite_triple_tests`)
//!
//! ## Boundary
//!
//! Changes to the SQLite triple end path should normally update tests here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
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

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
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
        id: id.into(),
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

fn add_open(store: &SqliteStore, bank: &str, subject: &str, id: &str, object: &str) {
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
fn triple_end_closes_open_row_on_both_axes() {
    let s = store();
    let bank = unique("b-end");
    let subject = unique("s-end");
    add_open(&s, &bank, &subject, "e1", "Boston");
    let out = s
        .triple_end(
            &bank,
            &subject,
            "lives_in",
            None,
            "2025-06-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        )
        .expect("end");
    assert_eq!(out.closed_ids, vec!["e1".to_owned()]);
    assert!(!out.already_ended);
    assert!(out.replacement_id.is_none(), "{:?}", out.replacement_id);
    let row = s.get_triple(&bank, "e1").expect("get").expect("row");
    assert_eq!(row.valid_until.as_deref(), Some("2025-06-01T00:00:00Z"));
    assert_eq!(row.tx_until.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert!(!row.is_open(), "{row:?}");
}

#[test]
fn triple_end_at_row_tx_from_stores_one_second_later() {
    let s = store();
    let bank = unique("b-same");
    let subject = unique("s-same");
    let stamp = "2025-05-01T00:00:00Z";
    s.commit_triple_add(
        &carrier(
            &format!("item-{bank}-e1"),
            &bank,
            "Boston",
            OPEN_FROM,
            stamp,
        ),
        &edge(&bank, &subject, "e1", "Boston", OPEN_FROM, stamp),
        true,
        "agent",
    )
    .expect("add");
    s.triple_end(
        &bank,
        &subject,
        "lives_in",
        None,
        "2025-04-01T00:00:00Z",
        stamp,
    )
    .expect("end");
    let row = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some("2025-05-01T00:00:01Z"),
        "close lands one second on"
    );
    let at_creation = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some(stamp),
            TimeAxis::Transaction,
        ))
        .unwrap();
    assert_eq!(at_creation.len(), 1, "row stays visible at {stamp}");
    let a_second_on = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some("2025-05-01T00:00:01Z"),
            TimeAxis::Transaction,
        ))
        .unwrap();
    assert!(a_second_on.is_empty(), "{a_second_on:?}");
}

#[test]
fn triple_end_second_call_reports_already_ended() {
    let s = store();
    let bank = unique("b-noop");
    let subject = unique("s-noop");
    add_open(&s, &bank, &subject, "e1", "Boston");
    let first = s
        .triple_end(
            &bank,
            &subject,
            "lives_in",
            None,
            "2025-06-01T00:00:00Z",
            "2025-06-02T00:00:00Z",
        )
        .unwrap();
    assert_eq!(first.closed_ids, vec!["e1".to_owned()]);
    let again = s
        .triple_end(
            &bank,
            &subject,
            "lives_in",
            None,
            "2025-07-01T00:00:00Z",
            "2025-07-02T00:00:00Z",
        )
        .unwrap();
    assert!(again.already_ended);
    assert!(again.closed_ids.is_empty(), "{:?}", again.closed_ids);
    let row = s.get_triple(&bank, "e1").unwrap().unwrap();
    assert_eq!(row.valid_until.as_deref(), Some("2025-06-01T00:00:00Z"));
}

#[test]
fn triple_end_without_open_row_reports_already_ended() {
    let s = store();
    let bank = unique("b-none");
    add_open(&s, &bank, "somebody-else", "e1", "Boston");
    let out = s
        .triple_end(
            &bank,
            "nobody",
            "lives_in",
            None,
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap();
    assert!(out.already_ended);
    assert!(out.closed_ids.is_empty(), "{:?}", out.closed_ids);
}

#[test]
fn triple_end_object_filter_skips_other_values() {
    let s = store();
    let bank = unique("b-obj");
    let subject = unique("s-obj");
    add_open(&s, &bank, &subject, "e1", "Boston");
    let miss = s
        .triple_end(
            &bank,
            &subject,
            "lives_in",
            Some("Chicago"),
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap();
    assert!(miss.already_ended, "{miss:?}");
    assert!(miss.closed_ids.is_empty(), "{:?}", miss.closed_ids);
    let hit = s
        .triple_end(
            &bank,
            &subject,
            "lives_in",
            Some("Boston"),
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap();
    assert_eq!(hit.closed_ids, vec!["e1".to_owned()]);
    assert!(!hit.already_ended);
}

#[test]
fn triple_end_rejects_blank_pattern_and_blank_stamps() {
    let s = store();
    let bank = unique("b-req");
    let no_bank = s
        .triple_end(
            "",
            "Alice",
            "lives_in",
            None,
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap_err();
    assert_eq!(no_bank.code, ErrorCode::InvalidArgument);
    let no_subject = s
        .triple_end(
            &bank,
            "",
            "lives_in",
            None,
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap_err();
    assert_eq!(no_subject.code, ErrorCode::InvalidArgument);
    let no_predicate = s
        .triple_end(
            &bank,
            "Alice",
            "",
            None,
            "2025-06-01T00:00:00Z",
            "2025-06-01T00:00:00Z",
        )
        .unwrap_err();
    assert_eq!(no_predicate.code, ErrorCode::InvalidArgument);
    let no_valid_until = s
        .triple_end(&bank, "Alice", "lives_in", None, "", "2025-06-01T00:00:00Z")
        .unwrap_err();
    assert_eq!(no_valid_until.code, ErrorCode::InvalidArgument);
    let no_tx_until = s
        .triple_end(&bank, "Alice", "lives_in", None, "2025-06-01T00:00:00Z", "")
        .unwrap_err();
    assert_eq!(no_tx_until.code, ErrorCode::InvalidArgument);
}

#[test]
fn query_triples_valid_axis_brackets_the_interval() {
    let s = store();
    let bank = unique("b-qv");
    let subject = unique("s-qv");
    add_open(&s, &bank, &subject, "e1", "Boston");
    s.triple_end(
        &bank,
        &subject,
        "lives_in",
        None,
        "2025-01-01T00:00:00Z",
        "2025-01-01T00:00:00Z",
    )
    .unwrap();
    let inside = s
        .query_triples(&query(
            &bank,
            Some(&subject),
            Some("lives_in"),
            None,
            Some("2024-06-01T00:00:00Z"),
            TimeAxis::Valid,
        ))
        .unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].object, "Boston");
    let before = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some("2023-06-01T00:00:00Z"),
            TimeAxis::Valid,
        ))
        .unwrap();
    assert!(before.is_empty(), "{before:?}");
    let after = s
        .query_triples(&query(
            &bank,
            None,
            None,
            None,
            Some("2025-06-01T00:00:00Z"),
            TimeAxis::Valid,
        ))
        .unwrap();
    assert!(after.is_empty(), "{after:?}");
}
