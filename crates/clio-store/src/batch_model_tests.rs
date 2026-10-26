// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite-side atomic batch executor tests:
//! all-or-nothing commit, rollback-on-failure with zero state change, the
//! operation cap, and every `BatchWrite` variant's bank tagging
//! The persistence contract those tests exist to pin.
//!
//! ## Owns
//! - SQLite `apply_batch` behavioral assertions and `BatchWrite` variant coverage
//!
//! ## Does not own
//! - Shared fixtures (see `batch_test_support_tests`)
//! - Batch persona-audit and record-context coverage (see `batch_tests_persona`)
//! - Postgres parity tests (see `batch_pg_tests`)
//! - MCP-level batch op decoding (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite, MAX_BATCH_OPS};
use crate::model::{AssocEdgeRecord, AssocKind, ItemInspectFilter};
use crate::store::Store;
use clio_types::{EpisodicType, ErrorCode, ItemKind};

use crate::batch_test_support_tests::{
    assert_all_or_nothing, extended_writes, item, record_shaped_writes, store, triple, triple_item,
};
use crate::test_unique::unique;

#[test]
fn sqlite_batch_is_atomic() {
    assert_all_or_nothing(&store());
}

#[test]
fn sqlite_canonical_put_effect_supersedes_in_transaction() {
    let store = store();
    let bank = unique("b");
    let key = unique("k");
    let subject = format!("canonical:{key}");
    let mut first = item(&unique("cq1"), &bank);
    first.subject_id = subject.clone();
    first.gist = Some("v1".into());
    first.valid_from = "2026-03-01T00:00:00Z".into();
    first.tx_from = "2026-03-01T00:00:00Z".into();
    let mut second = item(&unique("cq2"), &bank);
    second.subject_id = subject.clone();
    second.gist = Some("v2".into());
    second.valid_from = "2026-03-02T00:00:00Z".into();
    second.tx_from = "2026-03-02T00:00:00Z".into();
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
        .unwrap();
    // One active revision; the first is closed in-transaction, not deleted.
    let active = store
        .get_item_by_subject(&bank, &subject, None)
        .unwrap()
        .expect("active canonical");
    assert_eq!(active.id, second.id);
    assert!(active.tx_until.is_none());
    let closed = store.get_memory_item(&bank, &first.id).unwrap().unwrap();
    assert!(closed.tx_until.is_some());
    assert!(closed.valid_until.is_some());
}

#[test]
fn sqlite_batch_supersession_and_edge_writes() {
    let store = store();
    let bank = unique("b");
    let a = unique("a");
    store.create_memory_item(&item(&a, &bank), "agent").unwrap();
    let b = unique("b2");
    let writes = vec![
        BatchWrite::CloseItem {
            bank_id: bank.clone(),
            item_id: a.clone(),
            valid_until: "2026-01-02T00:00:00Z".into(),
            tx_until: "2026-01-02T00:00:00Z".into(),
            replacement_id: Some(b.clone()),
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item(&b, &bank),
            actor: "agent".into(),
        },
        BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: unique("edge"),
                bank_id: bank.clone(),
                source_id: a.clone(),
                target_id: b.clone(),
                assoc_kind: AssocKind::Explicit,
                relationship: Some("replaced_by".into()),
                weight: 0.5,
                last_updated_at: "2026-01-02T00:00:00Z".into(),
            },
            actor: "agent".into(),
        },
        BatchWrite::AuditEvent {
            bank_id: bank.clone(),
            operation: "batch_test".into(),
            item_id: Some(b.clone()),
            actor: "agent".into(),
            detail: serde_json::json!({ "k": "v" }),
        },
    ];
    store.apply_batch(&writes).unwrap();
    let prior = store.get_memory_item(&bank, &a).unwrap().unwrap();
    assert_eq!(prior.tx_until.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert!(store.get_memory_item(&bank, &b).unwrap().is_some());
    assert_eq!(store.list_assoc_edges(&bank, None).unwrap().len(), 1);
    let trail = store.audit_trail(&bank, &b).unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "batch_test"));
}

#[test]
fn sqlite_batch_triple_commit_rolls_back_together() {
    let store = store();
    let bank = unique("b");
    let good = unique("good");
    let dup = unique("dup");
    let writes = vec![
        BatchWrite::CommitTriple {
            item: triple_item(&good, &bank),
            edge: triple(&bank, &good),
            supersede: true,
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item(&dup, &bank),
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item(&dup, &bank), // duplicate id → whole batch fails
            actor: "agent".into(),
        },
    ];
    let err = store.apply_batch(&writes).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // Verification rule: zero state change after the aborted batch.
    assert!(
        store
            .get_triple(&bank, &format!("edge-{good}"))
            .unwrap()
            .is_none()
    );
    let mut filter = ItemInspectFilter::new(&bank);
    filter.include_discarded = true;
    assert_eq!(store.count_items(&filter).unwrap(), 0);
}

#[test]
#[allow(clippy::float_cmp)] // exact restored values from the batch payload
fn sqlite_batch_extended_import_variants() {
    let store = store();
    let bank = unique("b");
    store.apply_batch(&extended_writes(&bank)).unwrap();

    // Forced item overwrite applied the new gist.
    let rows = store.inspect_items(&ItemInspectFilter::new(&bank)).unwrap();
    let upd = rows
        .iter()
        .find(|r| r.item_id.starts_with("upd-"))
        .expect("upserted item listed");
    let restored = store.get_memory_item(&bank, &upd.item_id).unwrap().unwrap();
    assert_eq!(restored.gist.as_deref(), Some("updated gist"));

    // Opaque row stores the raw ciphertext and keeps its category.
    let opaque = rows
        .iter()
        .find(|r| r.item_id.starts_with("opq-"))
        .expect("opaque row listed");
    assert_eq!(opaque.category.as_deref(), Some("schema"));
    assert_eq!(
        store
            .inspect_content_ciphertext(&bank, &opaque.item_id)
            .unwrap()
            .as_deref(),
        Some("opaque-cipher")
    );

    // Preference upsert + belief create with one appended entry.
    let pref =
        crate::persona_store::PersonaStore::get_preference(&store, &bank, "brevity_affinity")
            .unwrap()
            .unwrap();
    assert_eq!(pref.value, 0.7);
    let beliefs = crate::belief_store::BeliefStore::list_beliefs(&store, &bank).unwrap();
    assert_eq!(beliefs.len(), 1);
    assert_eq!(beliefs[0].confidence_history.len(), 2);
}

#[test]
fn batch_cap_rejects_oversized_batches() {
    let store = store();
    let bank = unique("b");
    let writes: Vec<BatchWrite> = (0..=MAX_BATCH_OPS)
        .map(|i| BatchWrite::CreateItem {
            item: item(&format!("itm-{i}"), &bank),
            actor: "agent".into(),
        })
        .collect();
    let err = store.apply_batch(&writes).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("cap"));
}

#[test]
fn sqlite_batch_record_shaped_writes() {
    let store = store();
    let bank = unique("b");
    store.apply_batch(&record_shaped_writes(&bank)).unwrap();
    let tasks = crate::history_store::HistoryStore::list_tasks(&store, &bank, 10).unwrap();
    assert_eq!(tasks.len(), 1);
    let failures = crate::history_store::HistoryStore::list_failures(&store, &bank, 10).unwrap();
    assert_eq!(failures.len(), 1);
    let doc = crate::persona_store::PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(!doc.stable.is_empty(), "{:?}", doc.stable);
}

#[test]
fn batch_assoc_edge_and_triple_carrier_validation_fails_closed() {
    let store = store();
    let bank = unique("b");
    // An assoc edge with an empty id fails closed inside the transaction.
    let bad_edge = BatchWrite::PutAssocEdge {
        edge: AssocEdgeRecord {
            id: String::new(),
            bank_id: bank.clone(),
            source_id: "a".into(),
            target_id: "b".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 0.5,
            last_updated_at: "2026-01-01T00:00:00Z".into(),
        },
        actor: "agent".into(),
    };
    assert_eq!(bad_edge.bank_id(), bank);
    let err = store.apply_batch(&[bad_edge]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // Out-of-range weight likewise.
    let bad_weight = BatchWrite::PutAssocEdge {
        edge: AssocEdgeRecord {
            id: unique("edge"),
            bank_id: bank.clone(),
            source_id: "a".into(),
            target_id: "b".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 1.5,
            last_updated_at: "2026-01-01T00:00:00Z".into(),
        },
        actor: "agent".into(),
    };
    let err = store.apply_batch(&[bad_weight]).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);

    // A triple carrier whose id does not match its edge fails closed.
    let carrier_id = unique("carrier");
    let mut carrier = triple_item(&carrier_id, &bank);
    carrier.id = format!("{carrier_id}-other");
    let err = store
        .apply_batch(&[BatchWrite::CommitTriple {
            item: carrier,
            edge: triple(&bank, &carrier_id),
            supersede: true,
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn batch_write_bank_ids_cover_every_variant() {
    // Every `BatchWrite` variant reports its bank for tenant-boundary checks.
    let mut writes = record_shaped_writes("bank-x");
    writes.extend(extended_writes("bank-x"));
    for write in &writes {
        assert_eq!(write.bank_id(), "bank-x");
    }
    let mut carrier = item("itm", "bank-y");
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(EpisodicType::Triple);
    let triple_write = BatchWrite::CommitTriple {
        item: carrier,
        edge: triple("bank-y", "itm"),
        supersede: true,
        actor: "agent".into(),
    };
    assert_eq!(triple_write.bank_id(), "bank-y");
    assert_eq!(
        BatchWrite::CloseItem {
            bank_id: "bank-z".into(),
            item_id: "itm".into(),
            valid_until: "v".into(),
            tx_until: "t".into(),
            replacement_id: None,
            actor: "agent".into(),
        }
        .bank_id(),
        "bank-z"
    );
    let edge_id = unique("edge");
    assert_eq!(
        BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: edge_id,
                bank_id: "bank-z".into(),
                source_id: "a".into(),
                target_id: "b".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: None,
                weight: 0.5,
                last_updated_at: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        }
        .bank_id(),
        "bank-z"
    );
    assert_eq!(
        BatchWrite::AuditEvent {
            bank_id: "bank-z".into(),
            operation: "op".into(),
            item_id: None,
            actor: "agent".into(),
            detail: serde_json::json!({}),
        }
        .bank_id(),
        "bank-z"
    );
    assert_eq!(
        BatchWrite::CanonicalPut {
            item: item("canon-itm", "bank-w"),
            key: "k".into(),
            actor: "agent".into(),
        }
        .bank_id(),
        "bank-w"
    );
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
