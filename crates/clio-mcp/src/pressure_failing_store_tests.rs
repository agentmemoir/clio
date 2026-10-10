// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `pressure` block's source-failure unit
//! tests: an erroring store double and the handler's drain `Err` path must
//! render the exact unavailable reason per component, never fail the call and
//! never answer zero.
//!
//! ## Owns
//! - The all-failing `Store` double and the second-scan-fails double
//! - Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The pressure block itself (`pressure`)
//! - The other pressure fixtures (`pressure_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the pressure block's unavailable shape changes; they
//! never change production behavior.
//!
//! Keep this module focused on the pressure block's failure contract.

use super::pressure_tests::{BANK, NOW, state, status_with};
use clio_store::{
    AssocEdgeRecord, AssocPolicy, ClaimedSourceState, CorrectionOutcome, CorrectionSpec,
    DistillIntent, DistilledUnitRecord, EmbeddingSpace, ItemAuditTrail, ItemIdentity,
    ItemInspectFilter, ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery, Store, VectorSlot,
};
use clio_types::{
    AmError, ErrorCode, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery,
    TripleRecord,
};
use clio_write::HubDistillEngine;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Every read and write fails with one storage error: drives the pressure
/// block's real source-failure paths (stored-unit read, hub scan, archive
/// scan), not argument validation.
struct FailingStore;

macro_rules! fail_every_call {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                let _ = ($($arg),*);
                Err(AmError::new(ErrorCode::Internal, "storage unavailable"))
            }
        )*
    };
}

impl Store for FailingStore {
    fail_every_call! {
        delete_item(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        create_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        get_memory_item(bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;
        get_snapshot(bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;
        get_gist(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        put_assoc_edge(edge: &AssocEdgeRecord) -> Result<(), AmError>;
        get_assoc_edge(bank_id: &str, edge_id: &str) -> Result<Option<AssocEdgeRecord>, AmError>;
        delete_assoc_edge(bank_id: &str, edge_id: &str) -> Result<bool, AmError>;
        list_assoc_edges(bank_id: &str, item_id: Option<&str>) -> Result<Vec<AssocEdgeRecord>, AmError>;
        reinforce_assoc_edge(bank_id: &str, a: &str, b: &str, now_millis: i64, eta: f64) -> Result<AssocEdgeRecord, AmError>;
        record_assoc_audit(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        record_audit_event(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        get_item_by_subject(bank_id: &str, subject_id: &str, as_of: Option<&str>) -> Result<Option<MemoryItem>, AmError>;
        upsert_vector(slot: &VectorSlot) -> Result<(), AmError>;
        delete_vector(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        knn(bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;
        embedding_space() -> Result<EmbeddingSpace, AmError>;
        upsert_subject_key_ref(subject_id: &str, dek_ref: &str, status: &str) -> Result<(), AmError>;
        inspect_content_ciphertext(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        upsert_memtree_node(node: &MemTreeNodeRecord) -> Result<(), AmError>;
        get_memtree_node(bank_id: &str, node_id: &str) -> Result<Option<MemTreeNodeRecord>, AmError>;
        query_memtree_nodes(query: &MemTreeQuery) -> Result<Vec<MemTreeNodeRecord>, AmError>;
        commit_triple_add(item: &MemoryItem, edge: &TripleRecord, supersede: bool, actor: &str) -> Result<TripleAddOutcome, AmError>;
        get_triple(bank_id: &str, edge_id: &str) -> Result<Option<TripleRecord>, AmError>;
        query_triples(query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError>;
        triple_end(bank_id: &str, subject: &str, predicate: &str, object: Option<&str>, valid_until: &str, tx_until: &str) -> Result<TripleEndOutcome, AmError>;
        invalidate_item(bank_id: &str, item_id: &str, valid_until: &str, tx_until: &str, replacement_id: Option<&str>, actor: &str) -> Result<TripleEndOutcome, AmError>;
        get_item_identity(bank_id: &str, item_id: &str) -> Result<Option<ItemIdentity>, AmError>;
        discard_item(bank_id: &str, item_id: &str, reason: &str, discarded_at: &str, actor: &str) -> Result<ItemIdentity, AmError>;
        item_is_archived(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        item_ids_in_window(bank_id: &str, axis: TimeAxis, start: &str, end: &str, limit: usize) -> Result<Vec<String>, AmError>;
        inspect_items(filter: &ItemInspectFilter) -> Result<Vec<ItemInspectRow>, AmError>;
        count_items(filter: &ItemInspectFilter) -> Result<usize, AmError>;
        correct_item(spec: &CorrectionSpec) -> Result<CorrectionOutcome, AmError>;
        audit_trail(bank_id: &str, item_id: &str) -> Result<ItemAuditTrail, AmError>;
        list_triples(bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
        commit_distilled_unit(item: &MemoryItem, record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        get_distilled_unit(bank_id: &str, item_id: &str) -> Result<Option<DistilledUnitRecord>, AmError>;
    }
}

impl clio_store::DistilledUnitStore for FailingStore {
    fail_every_call! {
        list_distilled_units(bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError>;
        revise_distilled_unit(record: &DistilledUnitRecord, new_item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_distilled_unit(record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        update_distilled_unit_if_unchanged(expected: &DistilledUnitRecord, updated: &DistilledUnitRecord, actor: &str) -> Result<bool, AmError>;
        distilled_unit_claim_state(bank_id: &str, claim_ids: &[String]) -> Result<Vec<ClaimedSourceState>, AmError>;
        write_distill_intent(bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;
        complete_distill_intent(bank_id: &str, hub_id: &str, revision_item_id: &str) -> Result<bool, AmError>;
        get_distill_intent(bank_id: &str, hub_id: &str) -> Result<Option<DistillIntent>, AmError>;
    }
}

/// The first audit page read answers an empty bank page, and every later read
/// fails: drives the archive count's second scan failure arm, which the
/// all-failing [`FailingStore`] cannot reach because its first scan already
/// fails.
struct SecondScanFails {
    page_reads: AtomicUsize,
}

impl Store for SecondScanFails {
    fn inspect_items(&self, _filter: &ItemInspectFilter) -> Result<Vec<ItemInspectRow>, AmError> {
        if self.page_reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Vec::new())
        } else {
            Err(AmError::new(ErrorCode::Internal, "second scan unavailable"))
        }
    }

    fail_every_call! {
        delete_item(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        create_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        get_memory_item(bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;
        get_snapshot(bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;
        get_gist(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        put_assoc_edge(edge: &AssocEdgeRecord) -> Result<(), AmError>;
        get_assoc_edge(bank_id: &str, edge_id: &str) -> Result<Option<AssocEdgeRecord>, AmError>;
        delete_assoc_edge(bank_id: &str, edge_id: &str) -> Result<bool, AmError>;
        list_assoc_edges(bank_id: &str, item_id: Option<&str>) -> Result<Vec<AssocEdgeRecord>, AmError>;
        reinforce_assoc_edge(bank_id: &str, a: &str, b: &str, now_millis: i64, eta: f64) -> Result<AssocEdgeRecord, AmError>;
        record_assoc_audit(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        record_audit_event(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        get_item_by_subject(bank_id: &str, subject_id: &str, as_of: Option<&str>) -> Result<Option<MemoryItem>, AmError>;
        upsert_vector(slot: &VectorSlot) -> Result<(), AmError>;
        delete_vector(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        knn(bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;
        embedding_space() -> Result<EmbeddingSpace, AmError>;
        upsert_subject_key_ref(subject_id: &str, dek_ref: &str, status: &str) -> Result<(), AmError>;
        inspect_content_ciphertext(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        upsert_memtree_node(node: &MemTreeNodeRecord) -> Result<(), AmError>;
        get_memtree_node(bank_id: &str, node_id: &str) -> Result<Option<MemTreeNodeRecord>, AmError>;
        query_memtree_nodes(query: &MemTreeQuery) -> Result<Vec<MemTreeNodeRecord>, AmError>;
        commit_triple_add(item: &MemoryItem, edge: &TripleRecord, supersede: bool, actor: &str) -> Result<TripleAddOutcome, AmError>;
        get_triple(bank_id: &str, edge_id: &str) -> Result<Option<TripleRecord>, AmError>;
        query_triples(query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError>;
        triple_end(bank_id: &str, subject: &str, predicate: &str, object: Option<&str>, valid_until: &str, tx_until: &str) -> Result<TripleEndOutcome, AmError>;
        invalidate_item(bank_id: &str, item_id: &str, valid_until: &str, tx_until: &str, replacement_id: Option<&str>, actor: &str) -> Result<TripleEndOutcome, AmError>;
        get_item_identity(bank_id: &str, item_id: &str) -> Result<Option<ItemIdentity>, AmError>;
        discard_item(bank_id: &str, item_id: &str, reason: &str, discarded_at: &str, actor: &str) -> Result<ItemIdentity, AmError>;
        item_is_archived(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        item_ids_in_window(bank_id: &str, axis: TimeAxis, start: &str, end: &str, limit: usize) -> Result<Vec<String>, AmError>;
        count_items(filter: &ItemInspectFilter) -> Result<usize, AmError>;
        correct_item(spec: &CorrectionSpec) -> Result<CorrectionOutcome, AmError>;
        audit_trail(bank_id: &str, item_id: &str) -> Result<ItemAuditTrail, AmError>;
        list_triples(bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
        commit_distilled_unit(item: &MemoryItem, record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        get_distilled_unit(bank_id: &str, item_id: &str) -> Result<Option<DistilledUnitRecord>, AmError>;
    }
}

impl clio_store::DistilledUnitStore for SecondScanFails {
    fail_every_call! {
        list_distilled_units(bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError>;
        revise_distilled_unit(record: &DistilledUnitRecord, new_item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_distilled_unit(record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        update_distilled_unit_if_unchanged(expected: &DistilledUnitRecord, updated: &DistilledUnitRecord, actor: &str) -> Result<bool, AmError>;
        distilled_unit_claim_state(bank_id: &str, claim_ids: &[String]) -> Result<Vec<ClaimedSourceState>, AmError>;
        write_distill_intent(bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;
        complete_distill_intent(bank_id: &str, hub_id: &str, revision_item_id: &str) -> Result<bool, AmError>;
        get_distill_intent(bank_id: &str, hub_id: &str) -> Result<Option<DistillIntent>, AmError>;
    }
}

/// The archive count runs the audit twice under the two band floors. When the
/// first scan answers and the second fails, the block renders its unavailable
/// reason; it never reports a partial count from the scan that succeeded.
#[test]
fn a_failing_second_archive_scan_still_renders_unavailable() {
    let s = state();
    let mut scoped = (*s).clone();
    scoped.store = Arc::new(SecondScanFails {
        page_reads: AtomicUsize::new(0),
    });

    let out = super::archive_candidates(&scoped, BANK, NOW, super::ARCHIVE_SCAN_BUDGET);
    assert_eq!(
        out,
        json!({"unavailable": "hygiene archive scan failed (internal)"}),
        "{out}"
    );
}

#[test]
fn failing_sources_render_the_exact_unavailable_reason() {
    let s = state();
    let mut scoped = (*s).clone();
    let failing: Arc<dyn Store> = Arc::new(FailingStore);
    scoped.store = failing.clone();
    // The engine is wired, so the hub scan's own failure is the source, not a
    // missing engine.
    scoped.hub = Some(Arc::new(HubDistillEngine::with_clock(
        failing,
        s.env.clone(),
        AssocPolicy::default(),
        0,
    )));
    // The handler's real drain Err path: one failed read feeds both blocks.
    let drain: Result<clio_index::IndexStatus, AmError> =
        Err(AmError::new(ErrorCode::Internal, "queue unavailable"));
    let out = super::block(&scoped, BANK, &status_with(1, 0), NOW, &drain);
    assert_eq!(
        out["stale_units"],
        json!({
            "unavailable": "stored stale state read failed (internal)",
            "source": "stored_state"
        }),
        "{out}"
    );
    assert_eq!(
        out["eligible_hubs"],
        json!({"unavailable": "hub eligibility scan failed (internal)"}),
        "{out}"
    );
    assert_eq!(
        out["archive_candidates"],
        json!({"unavailable": "hygiene archive scan failed (internal)"}),
        "{out}"
    );
    assert_eq!(
        out["pending_jobs"],
        json!({"unavailable": "index drain status failed (internal)"}),
        "{out}"
    );
    assert_eq!(
        out["retry_jobs"],
        json!({"unavailable": "index drain status failed (internal)"}),
        "{out}"
    );
    // A component that never reads the store keeps its count.
    assert_eq!(out["dirty_ancestors"], json!({"count": 1}), "{out}");
    // The index copy renders the same failed read as its own unavailable
    // marker, so the call still returns ok.
    let marker = scoped.index_status_value(&drain);
    assert_eq!(
        marker,
        json!({"unavailable": "index drain status failed (internal)"}),
        "{marker}"
    );
}

impl clio_store::TripleAssertion for FailingStore {}
impl clio_store::TripleAssertion for SecondScanFails {}
