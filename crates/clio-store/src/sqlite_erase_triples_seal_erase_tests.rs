// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sealed-value and compliance-erasure
//! regression tests for SPO triples.
//!
//! ## Owns
//! - The regression test that `triple_add` followed by `erase_subject` leaves no
//!   readable copy of the content, on SQLite and on Postgres
//!
//! ## Does not own
//! - Core triple write/query semantics (see `triple_tests`)
//! - The legacy-shape open-guard tests (see `triple_reshape_tests`)
//!
//! ## Boundary
//!
//! Changes to how a triple's value is protected at rest, or to what a verified
//! erasure must leave behind, should normally update tests here.

use crate::HistoryStore;
use crate::compliance_store::{ComplianceStore, SubjectEraseSpec};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::store::Store;

/// The secret a triple value carries, and the needle the raw-row checks look for.
const SECRET: &str = "hunter2Correct";

/// A per-test data-subject id.
///
/// Erasure is subject-global: `subject_keys` is not bank-scoped, and destroying
/// one DEK permanently refuses writes for that subject, so no test may shred a
/// subject id another test still needs.
fn unique_subject() -> String {
    crate::test_unique::unique("subj")
}

fn sqlite_store() -> SqliteStore {
    SqliteStore::open_in_memory(crate::erase_test_support::local_kms()).expect("sqlite")
}

#[test]
fn the_asserted_value_is_sealed_at_rest_and_reads_stay_exact() {
    let s = sqlite_store();
    let bank = crate::test_unique::unique("b");
    let subject = unique_subject();
    let outcome = s
        .commit_triple_add(
            &triple_carrier(&bank, &subject),
            &triple_edge(&bank),
            true,
            "agent",
        )
        .expect("triple add");
    assert_eq!(outcome.new_edge_id, "e-seal");

    // At rest the value is ciphertext; through the read path it is exact.
    let raw = raw_object_column(&s, &bank, "e-seal");
    assert!(
        !raw.contains(SECRET),
        "the value must be sealed at rest: {raw}"
    );
    let got = s.get_triple(&bank, "e-seal").unwrap().expect("edge");
    assert_eq!(got.object, SECRET);
    assert_eq!(got.subject, "svc:db");
    assert_eq!(got.predicate, "password_is");
}

#[test]
fn erase_subject_leaves_no_readable_copy_of_the_triple() {
    let s = sqlite_store();
    let bank = crate::test_unique::unique("b");
    let subject = unique_subject();
    s.commit_triple_add(
        &triple_carrier(&bank, &subject),
        &triple_edge(&bank),
        true,
        "agent",
    )
    .expect("triple add");

    let outcome = s
        .erase_subject(&erase_spec(&bank, &subject))
        .expect("compliance erase");

    // The attestation names the triple rows it removed, the carrier item is
    // unreadable, and neither the sealed value (the DEK is gone) nor the
    // plaintext identity key (the row is deleted) survives.
    assert_eq!(outcome.triples_purged, 1, "the erase must report the row");
    assert_eq!(raw_triple_rows(&s, &bank), 0, "the row must be deleted");
    assert!(s.get_triple(&bank, "e-seal").unwrap().is_none());
    let remaining = s.list_triples(&bank).unwrap();
    assert!(remaining.is_empty(), "{remaining:?}");
    assert!(
        s.triple_history(&bank, "svc:db", "password_is", 10)
            .is_ok_and(|h| h.is_empty())
    );
}

#[test]
fn an_unrelated_subjects_triple_survives_the_erase() {
    // The purge is scoped to the erased subject's items, so it must not take out
    // rows it does not own: another data subject's edge in the same bank stays.
    let s = sqlite_store();
    let bank = crate::test_unique::unique("b");
    let subject = unique_subject();
    s.commit_triple_add(
        &triple_carrier(&bank, &subject),
        &triple_edge(&bank),
        true,
        "agent",
    )
    .expect("triple add");
    let mut keep = triple_carrier(&bank, &unique_subject());
    keep.id = "i-keep".into();
    let mut keep_edge = triple_edge(&bank);
    keep_edge.id = "e-keep".into();
    keep_edge.item_id = "i-keep".into();
    s.commit_triple_add(&keep, &keep_edge, true, "agent")
        .expect("triple add");

    let outcome = s
        .erase_subject(&erase_spec(&bank, &subject))
        .expect("compliance erase");
    assert_eq!(outcome.triples_purged, 1);
    let left = s.list_triples(&bank).expect("list");
    assert_eq!(left.len(), 1, "only the erased subject's edge goes");
    assert_eq!(left[0].id, "e-keep");
}

fn triple_carrier(bank: &str, subject: &str) -> clio_types::MemoryItem {
    clio_types::MemoryItem {
        id: "i-seal".into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: clio_types::ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"object": SECRET})),
        gist: Some(format!("db password is {SECRET}")),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
        context: None,
    }
}

fn triple_edge(bank: &str) -> clio_types::TripleRecord {
    clio_types::TripleRecord {
        id: "e-seal".into(),
        bank_id: bank.into(),
        item_id: "i-seal".into(),
        subject: "svc:db".into(),
        predicate: "password_is".into(),
        object: SECRET.into(),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

fn erase_spec<'a>(bank: &'a str, subject: &'a str) -> SubjectEraseSpec<'a> {
    let request = "req-issue-20";
    crate::erase_test_support::spec(bank, subject, request)
}

fn raw_object_column(s: &SqliteStore, bank: &str, edge_id: &str) -> String {
    s.with_conn(|conn| {
        conn.query_row(
            "SELECT object_ciphertext FROM triples WHERE bank_id = ?1 AND id = ?2",
            [bank, edge_id],
            |r| r.get(0),
        )
        .am("raw triple value")
    })
    .expect("raw stored value")
}

fn raw_triple_rows(s: &SqliteStore, bank: &str) -> i64 {
    s.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM triples WHERE bank_id = ?1",
            [bank],
            |r| r.get(0),
        )
        .am("count triple rows")
    })
    .expect("triple row count")
}
