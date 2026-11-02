// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite triple-history read bodies.
//!
//! ## Owns
//! - `triple_history_impl`, `triple_history_at_impl`, `record_from_row`, and
//!   carrier-context attachment
//!
//! ## Does not own
//! - Task/failure history reads (`sqlite_history_read`) or triple writes
//!   (`sqlite_triple`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![allow(clippy::assert_is_empty)]

use crate::migrate::IntoAmError;
use crate::sqlite::sqlite_history_tests::{item, store};
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, EpistemicKind, TimeAxis, TripleRecord};

fn edge(bank: &str, item_id: &str, subject: &str) -> TripleRecord {
    TripleRecord {
        id: unique("triple-edge"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: subject.into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: None,
    }
}

fn seed(store: &crate::sqlite::SqliteStore, bank: &str, subject: &str) -> String {
    let item_id = unique("triple-carrier");
    let mut carrier = item(&item_id, bank, EpisodicType::Triple, false);
    carrier.context = Some("carrier ctx".into());
    store
        .commit_triple_add(&carrier, &edge(bank, &item_id, subject), false, "agent")
        .unwrap();
    item_id
}

#[test]
#[allow(clippy::too_many_lines)]
fn triple_history_opens_asserted_value_and_carrier_context() {
    let store = store();
    let bank = unique("triple-history");
    seed(&store, &bank, "Alice");

    let history = store
        .triple_history_impl(&bank, "Alice", "lives_in", 10)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].object, "Boston");
    assert_eq!(history[0].context.as_deref(), Some("carrier ctx"));

    let valid = store
        .triple_history_at_impl(
            &bank,
            "Alice",
            "lives_in",
            "2026-06-01T00:00:00Z",
            TimeAxis::Valid,
            10,
        )
        .unwrap();
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].object, "Boston");
    assert_eq!(valid[0].context.as_deref(), Some("carrier ctx"));

    let transaction = store
        .triple_history_at_impl(
            &bank,
            "Alice",
            "lives_in",
            "2026-06-01T00:00:00Z",
            TimeAxis::Transaction,
            10,
        )
        .unwrap();
    assert_eq!(transaction.len(), 1);

    let earlier = store
        .triple_history_at_impl(
            &bank,
            "Alice",
            "lives_in",
            "2026-05-01T00:00:00Z",
            TimeAxis::Valid,
            10,
        )
        .unwrap();
    assert!(earlier.is_empty());

    let unknown = store
        .triple_history_impl(&bank, "Alice", "unknown", 10)
        .unwrap();
    assert!(unknown.is_empty());
}

#[test]
fn carrier_cipher_from_another_row_still_reads_without_context() {
    let store = store();
    let bank = unique("triple-binding");
    let first = seed(&store, &bank, "Alice");
    let second = seed(&store, &bank, "Bob");

    let cipher: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT content_ciphertext FROM items WHERE id = ?1",
                rusqlite::params![first],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '{cipher}' WHERE id = '{second}'"
        ))
        .unwrap();

    let history = store
        .triple_history_impl(&bank, "Bob", "lives_in", 10)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].object, "Boston");
    assert_eq!(history[0].context, None);
}
