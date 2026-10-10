// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the canonical single-slot
//! adapter: bi-temporal supersession and `as_of` historical reads.
//!
//! ## Owns
//! - Supersession-history and `as_of` assertions for `canonical_*`
//!
//! ## Does not own
//! - The canonical read adapter behavior itself (see `canonical.rs`)
//! - Shared store fixtures beyond this module's local helpers
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::canonical::{canonical_get, canonical_subject};
use crate::correct::{CorrectArgs, correct};
use clio_store::{BatchStore, BatchWrite, DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

use crate::test_unique::unique;

fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

fn slot_item(id: &str, bank: &str, key: &str, value: &str, timestamp: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: canonical_subject(key),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("tool_config").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({ "host": value })),
        gist: Some(value.to_owned()),
        valid_from: timestamp.to_owned(),
        valid_until: None,
        tx_from: timestamp.to_owned(),
        tx_until: None,
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
        context: None,
    }
}

/// Gate-equivalent canonical put: stage the atomic slot effect, return new id.
fn put(
    store: &SqliteStore,
    bank: &str,
    key: &str,
    value: &str,
    id: &str,
    timestamp: &str,
) -> String {
    let item = slot_item(id, bank, key, value, timestamp);
    store
        .apply_batch(&[BatchWrite::CanonicalPut {
            item: item.clone(),
            key: key.to_owned(),
            actor: "reviewer".into(),
        }])
        .unwrap();
    item.id
}

fn trail_has_operation(store: &SqliteStore, bank: &str, item_id: &str, op: &str) -> bool {
    store
        .audit_trail(bank, item_id)
        .unwrap()
        .events
        .iter()
        .any(|e| e.operation == op)
}

#[test]
fn put_supersedes_and_get_returns_latest() {
    let store = store();
    let bank = unique("b");
    let first = put(
        &store,
        &bank,
        "primary_database_host",
        "db-a",
        "itm-a",
        "2026-01-01T00:00:00Z",
    );
    let got = canonical_get(&store, &bank, "primary_database_host", None)
        .unwrap()
        .expect("active value");
    assert_eq!(got.value, "db-a");
    assert_eq!(got.item_id, first);
    assert_eq!(got.snapshot.as_ref().unwrap()["host"], "db-a");

    let second = put(
        &store,
        &bank,
        "primary_database_host",
        "db-b",
        "itm-b",
        "2026-01-02T00:00:00Z",
    );
    let got = canonical_get(&store, &bank, "primary_database_host", None)
        .unwrap()
        .unwrap();
    assert_eq!(got.value, "db-b");
    assert_eq!(got.item_id, second);

    // Bi-temporal supersession: prior closed on both axes, never deleted.
    let prior = store.get_memory_item(&bank, &first).unwrap().unwrap();
    assert_eq!(prior.tx_until.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert_eq!(prior.valid_until.as_deref(), Some("2026-01-02T00:00:00Z"));

    // Point-in-time read still sees the superseded revision.
    let historical = canonical_get(
        &store,
        &bank,
        "primary_database_host",
        Some("2026-01-01T12:00:00Z"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(historical.value, "db-a");
    assert_eq!(historical.item_id, first);

    assert!(trail_has_operation(&store, &bank, &second, "canonical_put"));
}

#[test]
fn canonical_put_batch_effect_supersedes_in_transaction() {
    let store = store();
    let bank = unique("b");
    let first = put(&store, &bank, "k", "v1", "itm-1", "2026-01-01T00:00:00Z");
    let second = slot_item("itm-2", &bank, "k", "v2", "2026-01-02T00:00:00Z");
    let writes = vec![BatchWrite::CanonicalPut {
        item: second,
        key: "k".into(),
        actor: "agent".into(),
    }];
    assert_eq!(writes[0].bank_id(), bank);
    store.apply_batch(&writes).unwrap();
    let got = canonical_get(&store, &bank, "k", None).unwrap().unwrap();
    assert_eq!(got.value, "v2");
    assert_eq!(got.item_id, "itm-2");
    let prior = store.get_memory_item(&bank, &first).unwrap().unwrap();
    assert!(prior.tx_until.is_some());
    assert!(trail_has_operation(&store, &bank, "itm-2", "canonical_put"));
}

#[test]
fn two_puts_one_key_in_one_batch_leave_one_active_revision() {
    let store = store();
    let bank = unique("b");
    let first = slot_item("itm-a", &bank, "k", "v1", "2026-01-01T00:00:00Z");
    let second = slot_item("itm-b", &bank, "k", "v2", "2026-01-02T00:00:00Z");
    store
        .apply_batch(&[
            BatchWrite::CanonicalPut {
                item: first,
                key: "k".into(),
                actor: "agent".into(),
            },
            BatchWrite::CanonicalPut {
                item: second,
                key: "k".into(),
                actor: "agent".into(),
            },
        ])
        .unwrap();
    // Exactly one active revision: the second put closes the first in-tx.
    let active = canonical_get(&store, &bank, "k", None).unwrap().unwrap();
    assert_eq!(active.item_id, "itm-b");
    assert_eq!(active.value, "v2");
    let closed = store.get_memory_item(&bank, "itm-a").unwrap().unwrap();
    assert!(closed.tx_until.is_some());
    let open = store.get_memory_item(&bank, "itm-b").unwrap().unwrap();
    assert!(open.tx_until.is_none());
}

#[test]
fn canonical_put_failure_after_close_rolls_back_the_slot() {
    let store = store();
    let bank = unique("b");
    // Active slot revision plus a colliding item id that makes the create fail
    // after the close has been staged.
    let prior = put(
        &store,
        &bank,
        "k",
        "v1",
        "itm-prior",
        "2026-01-01T00:00:00Z",
    );
    let collide = slot_item("itm-collide", &bank, "other", "x", "2026-01-01T00:00:00Z");
    store.create_memory_item(&collide, "agent").unwrap();

    let mut next = slot_item("itm-collide", &bank, "k", "v2", "2026-01-02T00:00:00Z");
    next.subject_id = canonical_subject("k");
    let err = store
        .apply_batch(&[BatchWrite::CanonicalPut {
            item: next,
            key: "k".into(),
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // The staged close rolled back with the failed create: the slot is intact.
    let row = store.get_memory_item(&bank, &prior).unwrap().unwrap();
    assert!(row.tx_until.is_none());
    assert_eq!(
        canonical_get(&store, &bank, "k", None)
            .unwrap()
            .unwrap()
            .item_id,
        prior
    );
}

#[test]
fn validation_guards_and_first_put() {
    let store = store();
    let err = canonical_get(&store, "", "k", None).unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
    let err = canonical_get(&store, "b", "  ", None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let item = slot_item("itm-1", "b1", "k", "v", "2026-01-01T00:00:00Z");
    store
        .apply_batch(&[BatchWrite::CanonicalPut {
            item,
            key: "k".into(),
            actor: "agent".into(),
        }])
        .unwrap();
    assert_eq!(
        canonical_get(&store, "b1", "k", None)
            .unwrap()
            .unwrap()
            .value,
        "v"
    );
}

#[test]
fn canonical_targets_still_route_through_correct() {
    // The canonical revision is a normal item row: discrete corrections apply.
    let store = store();
    let bank = unique("b");
    put(&store, &bank, "k", "v1", "itm-1", "2026-01-01T00:00:00Z");
    let c_args = CorrectArgs {
        store: &store,
        continuous: &store,
        bank_id: &bank,
        actor: "reviewer",
        item_id: "itm-1",
        new_value: "v1-fixed",
        reason: "typo",
        context: None,
        timestamp: "2026-01-03T00:00:00Z",
        new_item_id: "itm-2",
    };
    correct(&c_args).unwrap();
    assert!(
        store
            .get_memory_item(&bank, "itm-1")
            .unwrap()
            .unwrap()
            .tx_until
            .is_some()
    );
    assert!(trail_has_operation(&store, &bank, "itm-2", "correct"));
}
