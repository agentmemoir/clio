// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite batch executor tests owned by
//! `batch_sqlite`: every op arm the executor dispatches on, the cap guard, and
//! the drop-rollback that makes a failed op leave nothing behind.
//!
//! ## Owns
//! - Per-arm execution of `apply_one` through `apply_batch`, success and refusal
//!
//! ## Does not own
//! - The write contract itself (see `batch_model_tests`)
//! - Shared fixtures (see `batch_test_support_tests`)
//! - Postgres parity (see `batch_postgres_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. A new `BatchWrite` arm
//! should normally arrive here with a test that executes it.
//!
//! Keep this module focused on the executor's own dispatch.

use crate::batch_model::{BatchStore, BatchWrite, MAX_BATCH_OPS};
use crate::batch_test_support_tests::{
    extended_writes, item, record_shaped_writes, store, triple, triple_item,
};
use crate::belief_store::BeliefStore;
use crate::model::{AssocEdgeRecord, AssocKind, ItemInspectFilter};
use crate::persona_store::PersonaStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::ErrorCode;

#[test]
fn sqlite_batch_creates_closes_and_audits_in_one_transaction() {
    let store = store();
    let bank = unique("b-arm-create");
    let first = unique("item-a");
    let second = unique("item-b");
    let edge_id = unique("edge");
    store
        .apply_batch(&[
            BatchWrite::CreateItem {
                item: item(&first, &bank),
                actor: "agent".into(),
            },
            BatchWrite::CreateItem {
                item: item(&second, &bank),
                actor: "agent".into(),
            },
            BatchWrite::CloseItem {
                bank_id: bank.clone(),
                item_id: first.clone(),
                valid_until: "2026-02-01T00:00:00Z".into(),
                tx_until: "2026-02-01T00:00:00Z".into(),
                replacement_id: Some(second.clone()),
                actor: "agent".into(),
            },
            BatchWrite::PutAssocEdge {
                edge: AssocEdgeRecord {
                    id: edge_id,
                    bank_id: bank.clone(),
                    source_id: first.clone(),
                    target_id: second.clone(),
                    assoc_kind: AssocKind::Explicit,
                    relationship: Some("replaced_by".into()),
                    weight: 0.5,
                    last_updated_at: "2026-02-01T00:00:00Z".into(),
                },
                actor: "agent".into(),
            },
            BatchWrite::AuditEvent {
                bank_id: bank.clone(),
                operation: "batch_arm".into(),
                item_id: Some(second.clone()),
                actor: "agent".into(),
                detail: serde_json::json!({ "arm": "audit" }),
            },
        ])
        .expect("mixed batch commits");

    assert!(store.get_memory_item(&bank, &second).unwrap().is_some());
    let closed = store.get_memory_item(&bank, &first).unwrap().unwrap();
    assert_eq!(closed.tx_until.as_deref(), Some("2026-02-01T00:00:00Z"));
    assert_eq!(closed.valid_until.as_deref(), Some("2026-02-01T00:00:00Z"));
    assert_eq!(store.list_assoc_edges(&bank, None).unwrap().len(), 1);
    let trail = store.audit_trail(&bank, &second).unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "batch_arm"));
}

#[test]
fn sqlite_batch_canonical_put_closes_the_prior_slot_it_finds() {
    let store = store();
    let bank = unique("b-arm-canonical");
    let key = unique("k");
    let subject = format!("canonical:{key}");
    let mut first = item(&unique("cq1"), &bank);
    first.subject_id = subject.clone();
    first.gist = Some("v1".into());
    let mut second = item(&unique("cq2"), &bank);
    second.subject_id = subject.clone();
    second.gist = Some("v2".into());

    store
        .apply_batch(&[
            BatchWrite::CanonicalPut {
                item: first.clone(),
                key: key.clone(),
                actor: "agent".into(),
            },
            BatchWrite::CanonicalPut {
                item: second.clone(),
                key,
                actor: "agent".into(),
            },
        ])
        .expect("both puts commit");

    let active = store
        .get_item_by_subject(&bank, &subject, None)
        .unwrap()
        .expect("one active slot");
    assert_eq!(active.id, second.id, "the later put owns the slot");
    let prior = store.get_memory_item(&bank, &first.id).unwrap().unwrap();
    assert!(
        prior.tx_until.is_some() && prior.valid_until.is_some(),
        "the slot the first put owned is closed on both axes"
    );
    let trail = store.audit_trail(&bank, &first.id).unwrap();
    assert!(
        trail.events.iter().any(|e| e.operation == "canonical_put"),
        "the superseded slot carries the canonical_put marker"
    );
}

#[test]
fn sqlite_batch_triple_commit_seals_the_object_and_lands_the_edge() {
    let store = store();
    let bank = unique("b-arm-triple");
    let carrier = triple_item(&unique("tri"), &bank);
    store
        .apply_batch(&[BatchWrite::CommitTriple {
            item: carrier.clone(),
            edge: triple(&bank, &carrier.id),
            supersede: true,
            actor: "agent".into(),
        }])
        .expect("triple commits");
    let edges = store.list_triples(&bank).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].item_id, carrier.id);
    assert_eq!(edges[0].object, "o");
}

#[test]
fn sqlite_batch_triple_refuses_a_carrier_that_does_not_match_the_edge() {
    let store = store();
    let bank = unique("b-arm-triple-mismatch");
    let carrier = triple_item(&unique("tri"), &bank);
    let mut edge = triple(&bank, &carrier.id);
    edge.item_id = unique("other-item");
    let err = store
        .apply_batch(&[BatchWrite::CommitTriple {
            item: carrier,
            edge,
            supersede: false,
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.list_triples(&bank).unwrap().len(),
        0,
        "no triple row landed"
    );
}

#[test]
fn sqlite_batch_triple_refuses_a_carrier_without_the_triple_type() {
    let store = store();
    let bank = unique("b-arm-triple-type");
    let mut carrier = triple_item(&unique("tri"), &bank);
    carrier.episodic_type = None;
    let err = store
        .apply_batch(&[BatchWrite::CommitTriple {
            edge: triple(&bank, &carrier.id),
            item: carrier,
            supersede: false,
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.list_triples(&bank).unwrap().len(),
        0,
        "no triple row landed"
    );
}

#[test]
fn sqlite_batch_record_shaped_writes_commit_through_their_arms() {
    let store = store();
    let bank = unique("b-arm-record");
    let writes = record_shaped_writes(&bank);
    store.apply_batch(&writes).expect("record-shaped batch");
    for write in &writes {
        let Some(item) = write.created_item() else {
            continue;
        };
        assert!(
            store.get_memory_item(&bank, &item.id).unwrap().is_some(),
            "carrier {} landed",
            item.id
        );
    }
    let stable = writes
        .iter()
        .find_map(|w| match w {
            BatchWrite::PutStableEntry { entry, .. } => Some(entry.clone()),
            _ => None,
        })
        .expect("stable entry write");
    let history = store.stable_history(&bank, &stable.key).unwrap();
    assert_eq!(history.len(), 1, "one stable entry for the key");
    assert_eq!(history[0].value, "concise");
}

#[test]
fn sqlite_batch_extended_import_variants_commit_through_their_arms() {
    let store = store();
    let bank = unique("b-arm-extended");
    let writes = extended_writes(&bank);
    store.apply_batch(&writes).expect("extended batch");
    let belief = writes
        .iter()
        .find_map(|w| match w {
            BatchWrite::CreateBelief { belief, .. } => Some(belief.clone()),
            _ => None,
        })
        .expect("belief write");
    assert!(store.get_belief(&bank, &belief.id).unwrap().is_some());
}

#[test]
fn sqlite_batch_assoc_edge_refuses_an_empty_identity() {
    let store = store();
    let bank = unique("b-arm-edge-id");
    let err = store
        .apply_batch(&[BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: String::new(),
                bank_id: bank.clone(),
                source_id: "a".into(),
                target_id: "b".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: None,
                weight: 0.5,
                last_updated_at: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.list_assoc_edges(&bank, None).unwrap().len(),
        0,
        "no edge landed"
    );
}

#[test]
fn sqlite_batch_assoc_edge_refuses_a_weight_outside_the_unit_interval() {
    let store = store();
    let bank = unique("b-arm-edge-weight");
    let err = store
        .apply_batch(&[BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: unique("edge"),
                bank_id: bank.clone(),
                source_id: "a".into(),
                target_id: "b".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: None,
                weight: 1.5,
                last_updated_at: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert_eq!(
        store.list_assoc_edges(&bank, None).unwrap().len(),
        0,
        "no edge landed"
    );
}

#[test]
fn sqlite_batch_refuses_more_operations_than_the_cap() {
    let store = store();
    let bank = unique("b-arm-cap");
    let writes: Vec<BatchWrite> = (0..=MAX_BATCH_OPS)
        .map(|i| BatchWrite::CreateItem {
            item: item(&format!("cap-{i}-{}", unique("x")), &bank),
            actor: "agent".into(),
        })
        .collect();
    let err = store.apply_batch(&writes).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("cap"), "{}", err.message);
    let mut filter = ItemInspectFilter::new(&bank);
    filter.include_discarded = true;
    assert_eq!(store.count_items(&filter).unwrap(), 0);
}

#[test]
fn sqlite_batch_drops_every_earlier_write_when_a_later_op_fails() {
    let store = store();
    let bank = unique("b-arm-rollback");
    let dup = unique("dup");
    let err = store
        .apply_batch(&[
            BatchWrite::CreateItem {
                item: item(&unique("rb1"), &bank),
                actor: "agent".into(),
            },
            BatchWrite::CreateItem {
                item: item(&dup, &bank),
                actor: "agent".into(),
            },
            BatchWrite::CreateItem {
                item: item(&dup, &bank),
                actor: "agent".into(),
            },
        ])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let mut filter = ItemInspectFilter::new(&bank);
    filter.include_discarded = true;
    assert_eq!(store.count_items(&filter).unwrap(), 0);
}

#[test]
fn sqlite_batch_close_item_in_its_starting_second_widens_the_interval() {
    let store = store();
    let bank = unique("b-batch-close");
    store
        .apply_batch(&[BatchWrite::CreateItem {
            item: item("item-a", &bank),
            actor: "agent".into(),
        }])
        .expect("create");
    store
        .apply_batch(&[BatchWrite::CloseItem {
            bank_id: bank.clone(),
            item_id: "item-a".into(),
            valid_until: "2026-02-01T00:00:00Z".into(),
            tx_until: "2026-01-01T02:00:00+02:00".into(),
            replacement_id: None,
            actor: "agent".into(),
        }])
        .expect("close");
    let row = store.get_memory_item(&bank, "item-a").unwrap().unwrap();
    assert_eq!(row.tx_from, "2026-01-01T00:00:00Z");
    assert_eq!(
        row.tx_until.as_deref(),
        Some("2026-01-01T00:00:01Z"),
        "the batch close stores a canonical stamp one second on"
    );
    assert_eq!(row.valid_until.as_deref(), Some("2026-02-01T00:00:00Z"));
}
