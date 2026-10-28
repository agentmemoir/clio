// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite `Store` trait implementation surface: the item
//! lifecycle, association edges, audit rows, vector hooks, inspection, and the
//! logged removal and invalidation paths.
//!
//! ## Owns
//! - End-to-end assertions that every wired `Store` method answers with real
//!   persisted behavior on the SQLite backend
//!
//! ## Does not own
//! - Production behaviour
//! - SQL bodies that live in the `sqlite_*` implementation modules
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the SQLite `Store` wiring surface.

use crate::audit_model::CorrectionSpec;
use crate::dek::{DekProvider, LocalDevKms};
use crate::model::{
    AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, InspectValidity, ItemInspectFilter, VectorSlot,
};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use crate::triple_assertion::TripleAssertion;
use clio_types::{
    EpistemicKind, ItemKind, MemoryItem, SemanticCategory, TimeAxis, TripleEndOutcome, TripleQuery,
};
use serde_json::json;
use std::sync::Arc;

const T0: &str = "2026-01-01T00:00:00Z";
const T1: &str = "2026-02-01T00:00:00Z";

fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subject-{bank}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.6),
        source_ref: Some("store-test".into()),
        context: None,
        snapshot: Some(json!({"gist": gist})),
        gist: Some(gist.into()),
        valid_from: T0.into(),
        valid_until: None,
        tx_from: T0.into(),
        tx_until: None,
        created_at: T0.into(),
        updated_at: T0.into(),
    }
}

fn edge(id: &str, bank: &str, source: &str, target: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: source.into(),
        target_id: target.into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("related".into()),
        weight: 0.5,
        last_updated_at: T0.into(),
    }
}

fn unit_vector(axis: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; EMBEDDING_DIMS];
    embedding[axis] = 1.0;
    embedding
}

fn open() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

#[test]
fn triple_and_memtree_reads_answer_on_an_empty_store() {
    let store = open();
    let bank = unique("bank-triples");
    assert!(
        store
            .get_triple(&bank, &unique("absent"))
            .expect("get triple")
            .is_none(),
        "an unknown SPO edge must read as None"
    );
    assert!(
        store.list_triples(&bank).expect("list triples").is_empty(),
        "an empty bank must list no triples"
    );
    let query = TripleQuery {
        bank_id: bank.clone(),
        subject: None,
        predicate: None,
        object: None,
        as_of: None,
        time_axis: TimeAxis::Valid,
    };
    assert!(
        store.query_triples(&query).expect("query triples").is_empty(),
        "an empty bank must query to no triples"
    );
    store
        .triple_end(&bank, "subject", "predicate", None, T1, T1)
        .expect("a close on an unknown SPO head must still answer");
    assert!(
        !store
            .triple_asserted_ever(&bank, "subject", "predicate", None)
            .expect("asserted-ever read"),
        "nothing has ever been asserted on an empty store"
    );
    assert!(
        store
            .get_memtree_node(&bank, &unique("absent"))
            .expect("memtree read")
            .is_none(),
        "an unknown memtree node must read as None"
    );
}

#[test]
fn the_item_lifecycle_round_trips_through_the_wiring() {
    let store = open();
    let bank = unique("bank-lifecycle");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank, "first gist"), "agent")
        .expect("create");

    let read = store
        .get_memory_item(&bank, &id)
        .expect("read back")
        .expect("the item must exist");
    assert_eq!(read.gist.as_deref(), Some("first gist"), "{read:?}");

    let mut updated = read.clone();
    updated.gist = Some("second gist".into());
    updated.snapshot = Some(json!({"gist": "second gist"}));
    updated.updated_at = T1.into();
    store
        .update_memory_item(&updated, "agent")
        .expect("update");
    assert_eq!(
        store
            .get_gist(&bank, &id)
            .expect("gist read"),
        Some("second gist".into()),
        "the update must replace the gist"
    );
    assert_eq!(
        store
            .get_snapshot(&bank, &id)
            .expect("snapshot read"),
        Some(json!({"gist": "second gist"})),
        "the snapshot must survive the update"
    );
    let by_subject = store
        .get_item_by_subject(&bank, &format!("subject-{bank}"), None)
        .expect("subject read")
        .expect("the current revision must resolve");
    assert_eq!(by_subject.id, id, "{by_subject:?}");
    assert!(
        store
            .get_item_identity(&bank, &id)
            .expect("identity read")
            .is_some(),
        "the identity must resolve for an open item"
    );
}

#[test]
fn association_edges_round_trip_and_reinforce_in_place() {
    let store = open();
    let bank = unique("bank-assoc");
    let a = unique("a");
    let b = unique("b");
    let c = unique("c");
    for id in [&a, &b, &c] {
        store
            .create_memory_item(&item(id, &bank, "node"), "agent")
            .expect("create node");
    }
    let edge_id = unique("edge");
    store
        .put_assoc_edge(&edge(&edge_id, &bank, &a, &b))
        .expect("put edge");
    let read = store
        .get_assoc_edge(&bank, &edge_id)
        .expect("get edge")
        .expect("the edge must exist");
    assert_eq!(read.weight, 0.5, "{read:?}");

    let reinforced = store
        .reinforce_assoc_edge(&bank, &a, &b, 1_000, 0.75)
        .expect("reinforce");
    assert!(reinforced.weight > 0.5, "{reinforced:?}");

    let listed = store
        .list_assoc_edges(&bank, Some(&a))
        .expect("list edges");
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(
        listed
            .iter()
            .any(|e| e.id == edge_id && e.assoc_kind == AssocKind::Explicit),
        "{listed:?}"
    );
    assert!(
        listed
            .iter()
            .any(|e| e.assoc_kind == AssocKind::Coactivation),
        "the reinforce must have created the co-activation edge: {listed:?}"
    );

    assert!(
        store
            .delete_assoc_edge(&bank, &edge_id)
            .expect("delete edge"),
        "the edge delete must report true"
    );
    assert!(
        store
            .get_assoc_edge(&bank, &edge_id)
            .expect("get deleted edge")
            .is_none(),
        "the edge must be gone"
    );
    let listed = store
        .list_assoc_edges(&bank, Some(&a))
        .expect("list edges after delete");
    assert_eq!(listed.len(), 1, "{listed:?}");
}

#[test]
fn audit_rows_record_operations_and_corrections_build_a_trail() {
    let store = open();
    let bank = unique("bank-audit");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank, "first"), "agent")
        .expect("create");

    store
        .record_audit_event(&bank, "workspace.attest", Some(&id), "reviewer", &json!({"ok": true}))
        .expect("audit event");
    store
        .record_assoc_audit(&bank, "edge.prune", Some(&id), "agent", &json!({"weight": 0.5}))
        .expect("assoc audit");

    let mut corrected = item(&unique("live"), &bank, "second");
    corrected.tx_from = T1.into();
    corrected.valid_from = T1.into();
    corrected.created_at = T1.into();
    corrected.updated_at = T1.into();
    store
        .correct_item(&CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: id.clone(),
            new_item: corrected,
            reason: "belief changed".into(),
            actor: "agent".into(),
            timestamp: T1.into(),
        })
        .expect("correct");

    let trail = store.audit_trail(&bank, &id).expect("audit trail");
    assert!(
        !trail.events.is_empty(),
        "the prior item must keep its audit trail: {trail:?}"
    );
}

#[test]
fn vector_hooks_store_scan_and_remove_rows() {
    let store = open();
    let bank = unique("bank-vector");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank, "embedded"), "agent")
        .expect("create");
    store
        .upsert_vector(&VectorSlot {
            item_id: id.clone(),
            bank_id: bank.clone(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: unit_vector(0),
        })
        .expect("upsert vector");

    let space = store.embedding_space().expect("embedding space");
    assert_eq!(space.dims, EMBEDDING_DIMS, "{space:?}");
    let hits = store.knn(&bank, &unit_vector(0), 1).expect("knn");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, id, "{hits:?}");

    assert!(
        store.delete_vector(&bank, &id).expect("delete vector"),
        "the vector delete must report true"
    );
    assert!(
        !store.delete_vector(&bank, &id).expect("repeat delete"),
        "a repeated vector delete must report false"
    );
    assert!(
        store.knn(&bank, &unit_vector(0), 1).expect("knn").is_empty(),
        "no vector may remain"
    );
}

#[test]
fn inspection_reports_rows_and_windows_in_deterministic_pages() {
    let store = open();
    let bank = unique("bank-inspect");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank, "inspect me"), "agent")
        .expect("create");

    let filter = ItemInspectFilter::new(bank.clone());
    let rows = store.inspect_items(&filter).expect("inspect");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].item_id, id, "{rows:?}");
    assert_eq!(store.count_items(&filter).expect("count"), 1, "{rows:?}");

    let ids = store
        .item_ids_in_window(&bank, TimeAxis::Valid, T0, T1, 10)
        .expect("window read");
    assert_eq!(ids, vec![id.clone()], "{ids:?}");
    assert!(
        store
            .inspect_content_ciphertext(&bank, &id)
            .is_ok(),
        "the ciphertext inspect must answer for an existing item"
    );
    store
        .upsert_subject_key_ref(&format!("subject-{bank}"), "dek-ref-1", "active")
        .expect("subject key ref");
}

#[test]
fn discard_and_invalidate_close_rows_without_erasing_them() {
    let store = open();
    let bank = unique("bank-close");
    let discarded = unique("discarded");
    let invalidated = unique("invalidated");
    store
        .create_memory_item(&item(&discarded, &bank, "drop"), "agent")
        .expect("create discarded");
    store
        .create_memory_item(&item(&invalidated, &bank, "close"), "agent")
        .expect("create invalidated");

    store
        .discard_item(&bank, &discarded, "rotated away", T1, "agent")
        .expect("discard");
    assert!(
        !store.item_is_archived(&bank, &discarded).expect("archived read"),
        "a discard is not an archive"
    );

    let outcome: TripleEndOutcome = store
        .invalidate_item(&bank, &invalidated, T1, T1, None, "agent")
        .expect("invalidate");
    let _ = outcome;
    let closed = store
        .count_items(&{
            let mut filter = ItemInspectFilter::new(bank.clone());
            filter.validity = InspectValidity::Invalidated;
            filter
        })
        .expect("invalidated count");
    assert_eq!(closed, 1, "the invalidated item must be the only closed one");

    assert!(
        store
            .get_distilled_unit(&bank, &invalidated)
            .expect("distilled read")
            .is_none(),
        "an item without a distilled record must read as None"
    );
}