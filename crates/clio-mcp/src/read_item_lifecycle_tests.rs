// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the lifecycle stamping helper directly against a real
//! store: the two markers for a live, a discarded, and an archived item, the
//! payload's existing keys, and a store failure propagating as an error.
//!
//! ## Owns
//! - The three marker readings and the preserved-key check
//! - The all-failing `Store` double and the error-not-`false` contract
//!
//! ## Does not own
//! - The stamping itself (`read_item_lifecycle`)
//! - The handler-level reads that carry the flags (`read_tools_lifecycle_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the two markers and the failure contract.

use super::{ARCHIVED_KEY, DISCARDED_KEY, attach};
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_store::{
    AssocEdgeRecord, ClaimedSourceState, CorrectionOutcome, CorrectionSpec, DistillIntent,
    DistilledUnitRecord, EmbeddingSpace, ItemAuditTrail, ItemIdentity, ItemInspectFilter,
    ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery, Store, VectorSlot,
};
use clio_types::{
    AmError, ErrorCode, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery,
    TripleRecord,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_args(id: &str, text: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": "subj",
            "gist": text,
            "snapshot": {"entity": text, "amount": 1, "date": "2024-01-01"}
        },
        "category": "task_spec",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers {text} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent",
        "timestamp": "2024-01-01T00:00:00Z"
    })
}

/// Every call fails with one storage error, so a failed marker read is exercised
/// without a real storage fault.
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

impl clio_store::TripleAssertion for FailingStore {}

#[test]
fn a_live_item_reports_neither_marker_and_keeps_its_keys() {
    let s = state();
    let stored = call(&s, "store", store_args("itm-live", "live note"));
    assert_eq!(stored["ok"], json!(true), "store itm-live: {stored}");

    let mut payload = json!({"item_id": "itm-live", "gist": "keep me"});
    attach(&*s.store, "bank-a", "itm-live", &mut payload).expect("attach on a live item");

    assert_eq!(payload[DISCARDED_KEY], json!(false), "{payload}");
    assert_eq!(payload[ARCHIVED_KEY], json!(false), "{payload}");
    assert_eq!(payload["item_id"], json!("itm-live"), "{payload}");
    assert_eq!(payload["gist"], json!("keep me"), "{payload}");
}

#[test]
fn a_discarded_item_reports_the_discarded_marker() {
    let s = state();
    let stored = call(&s, "store", store_args("itm-gone", "gone note"));
    assert_eq!(stored["ok"], json!(true), "store itm-gone: {stored}");
    let discarded = call(
        &s,
        "discard",
        json!({
            "bank": "bank-a",
            "item_id": "itm-gone",
            "reason": "ops",
            "confirm": true,
            "timestamp": "2024-02-01T00:00:00Z"
        }),
    );
    assert_eq!(discarded["ok"], json!(true), "discard itm-gone: {discarded}");

    let mut payload = json!({"item_id": "itm-gone"});
    attach(&*s.store, "bank-a", "itm-gone", &mut payload).expect("attach on a discarded item");

    assert_eq!(payload[DISCARDED_KEY], json!(true), "{payload}");
    assert_eq!(payload[ARCHIVED_KEY], json!(false), "{payload}");
}

#[test]
fn an_archived_item_reports_the_archived_marker() {
    let s = state();
    let stored = call(&s, "store", store_args("itm-arc", "archivable note"));
    assert_eq!(stored["ok"], json!(true), "store itm-arc: {stored}");
    let cleaned = call(
        &s,
        "hygiene_clean",
        json!({
            "bank": "bank-a",
            "item_ids": ["itm-arc"],
            "action": "archive",
            "confirm": true,
            "timestamp": "2024-03-01T00:00:00Z"
        }),
    );
    assert_eq!(cleaned["ok"], json!(true), "archive itm-arc: {cleaned}");

    let mut payload = json!({"item_id": "itm-arc"});
    attach(&*s.store, "bank-a", "itm-arc", &mut payload).expect("attach on an archived item");

    assert_eq!(payload[ARCHIVED_KEY], json!(true), "{payload}");
    assert_eq!(payload[DISCARDED_KEY], json!(false), "{payload}");
}

#[test]
fn a_non_object_payload_is_left_alone() {
    let s = state();
    let stored = call(&s, "store", store_args("itm-array", "array note"));
    assert_eq!(stored["ok"], json!(true), "store itm-array: {stored}");

    let mut payload = json!(["not", "an", "object"]);
    attach(&*s.store, "bank-a", "itm-array", &mut payload).expect("attach on an array payload");

    assert_eq!(payload, json!(["not", "an", "object"]), "{payload}");
}

#[test]
fn a_store_failure_is_an_error_and_never_a_false() {
    let mut payload = json!({"item_id": "itm-1"});
    let failed = attach(&FailingStore, "bank-a", "itm-1", &mut payload);

    assert!(failed.is_err(), "a failed marker read must not answer false");
    assert!(payload.get(DISCARDED_KEY).is_none(), "{payload}");
    assert!(payload.get(ARCHIVED_KEY).is_none(), "{payload}");
}
