// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for durable-handoff tests of the retrieve
//! reinforcement hook: a mid-wave store failure must fail the retrieve
//! closed, and a successful wave must emit PII-safe reinforce telemetry.
//!
//! ## Owns
//! - The delegating `Store` double that faults a chosen reinforce call
//!
//! ## Does not own
//! - Production reinforce logic (`assoc`) or the weight formula (`clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::hybrid::HybridRetriever;
use crate::types::RetrieveRequest;
use clio_store::{
    AssocEdgeRecord, ClaimedSourceState, DistillIntent, DistilledUnitRecord, IndexStore,
    ItemIdentity, ItemInspectFilter, ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery,
    SqliteStore, Store, VectorSlot,
};
use clio_types::{
    AmError, ErrorCode, MemoryItem, TripleAddOutcome, TripleEndOutcome, TripleQuery, TripleRecord,
};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Delegating `Store` double: forwards to the real store, counts reinforce and
/// audit calls, and fails the `fail_on_call`-th reinforce (1-based; 0 = never).
struct FaultyStore {
    inner: Arc<dyn Store>,
    reinforce_calls: AtomicUsize,
    audits: AtomicUsize,
    fail_on_call: usize,
}

macro_rules! forward_store {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for FaultyStore {
    forward_store! {
        delete_item(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        create_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        get_memory_item(bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;
        get_snapshot(bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;
        get_gist(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        put_assoc_edge(edge: &AssocEdgeRecord) -> Result<(), AmError>;
        get_assoc_edge(bank_id: &str, edge_id: &str) -> Result<Option<AssocEdgeRecord>, AmError>;
        delete_assoc_edge(bank_id: &str, edge_id: &str) -> Result<bool, AmError>;
        list_assoc_edges(
            bank_id: &str,
            item_id: Option<&str>,
        ) -> Result<Vec<AssocEdgeRecord>, AmError>;
        upsert_vector(slot: &VectorSlot) -> Result<(), AmError>;
        delete_vector(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        knn(bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;
        embedding_space() -> Result<clio_store::EmbeddingSpace, AmError>;
        upsert_subject_key_ref(
            subject_id: &str,
            dek_ref: &str,
            status: &str,
        ) -> Result<(), AmError>;
        inspect_content_ciphertext(
            bank_id: &str,
            item_id: &str,
        ) -> Result<Option<String>, AmError>;
        upsert_memtree_node(node: &MemTreeNodeRecord) -> Result<(), AmError>;
        get_memtree_node(
            bank_id: &str,
            node_id: &str,
        ) -> Result<Option<MemTreeNodeRecord>, AmError>;
        query_memtree_nodes(
            query: &MemTreeQuery,
        ) -> Result<Vec<MemTreeNodeRecord>, AmError>;
        commit_triple_add(
            item: &MemoryItem,
            edge: &TripleRecord,
            supersede: bool,
            actor: &str,
        ) -> Result<TripleAddOutcome, AmError>;
        get_triple(bank_id: &str, edge_id: &str) -> Result<Option<TripleRecord>, AmError>;
        query_triples(query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError>;
        triple_end(
            bank_id: &str,
            subject: &str,
            predicate: &str,
            object: Option<&str>,
            valid_until: &str,
            tx_until: &str,
        ) -> Result<TripleEndOutcome, AmError>;
        invalidate_item(
            bank_id: &str,
            item_id: &str,
            valid_until: &str,
            tx_until: &str,
            replacement_id: Option<&str>,
            actor: &str,
        ) -> Result<TripleEndOutcome, AmError>;
        get_item_identity(
            bank_id: &str,
            item_id: &str,
        ) -> Result<Option<ItemIdentity>, AmError>;
        discard_item(
            bank_id: &str,
            item_id: &str,
            reason: &str,
            discarded_at: &str,
            actor: &str,
        ) -> Result<ItemIdentity, AmError>;
        inspect_items(
            filter: &ItemInspectFilter,
        ) -> Result<Vec<ItemInspectRow>, AmError>;
        count_items(filter: &ItemInspectFilter) -> Result<usize, AmError>;
        item_is_archived(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        item_ids_in_window(
            bank_id: &str,
            axis: clio_types::TimeAxis,
            start: &str,
            end: &str,
            limit: usize,
        ) -> Result<Vec<String>, AmError>;
        correct_item(
            spec: &clio_store::CorrectionSpec,
        ) -> Result<clio_store::CorrectionOutcome, AmError>;
        audit_trail(bank_id: &str, item_id: &str) -> Result<clio_store::ItemAuditTrail, AmError>;
        list_triples(bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
        commit_distilled_unit(
            item: &MemoryItem,
            record: &DistilledUnitRecord,
            actor: &str,
        ) -> Result<(), AmError>;
        get_distilled_unit(
            bank_id: &str,
            item_id: &str,
        ) -> Result<Option<DistilledUnitRecord>, AmError>;
    }

    fn reinforce_assoc_edge(
        &self,
        bank_id: &str,
        a: &str,
        b: &str,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError> {
        let call = self.reinforce_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_on_call {
            return Err(AmError::new(
                ErrorCode::Internal,
                "reinforce durable handoff failed",
            ));
        }
        self.inner
            .reinforce_assoc_edge(bank_id, a, b, now_millis, eta)
    }

    fn record_assoc_audit(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.audits.fetch_add(1, Ordering::SeqCst);
        self.inner
            .record_assoc_audit(bank_id, operation, item_id, actor, detail)
    }

    fn record_audit_event(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.inner
            .record_audit_event(bank_id, operation, item_id, actor, detail)
    }

    fn get_item_by_subject(
        &self,
        bank_id: &str,
        subject_id: &str,
        as_of: Option<&str>,
    ) -> Result<Option<MemoryItem>, AmError> {
        self.inner.get_item_by_subject(bank_id, subject_id, as_of)
    }
}

impl clio_store::DistilledUnitStore for FaultyStore {
    forward_store! {
        list_distilled_units(bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError>;
        revise_distilled_unit(
            record: &DistilledUnitRecord,
            new_item: &MemoryItem,
            actor: &str,
        ) -> Result<(), AmError>;
        update_distilled_unit(record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        update_distilled_unit_if_unchanged(
            expected: &DistilledUnitRecord,
            updated: &DistilledUnitRecord,
            actor: &str,
        ) -> Result<bool, AmError>;
        distilled_unit_claim_state(
            bank_id: &str,
            claim_ids: &[String],
        ) -> Result<Vec<ClaimedSourceState>, AmError>;
        write_distill_intent(bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;
        complete_distill_intent(
            bank_id: &str,
            hub_id: &str,
            revision_item_id: &str,
        ) -> Result<bool, AmError>;
        get_distill_intent(
            bank_id: &str,
            hub_id: &str,
        ) -> Result<Option<DistillIntent>, AmError>;
    }
}

fn faulty(real: &Arc<SqliteStore>, fail_on_call: usize) -> Arc<FaultyStore> {
    Arc::new(FaultyStore {
        inner: Arc::clone(real) as Arc<dyn Store>,
        reinforce_calls: AtomicUsize::new(0),
        audits: AtomicUsize::new(0),
        fail_on_call,
    })
}

fn retriever(real: &Arc<SqliteStore>, double: &Arc<FaultyStore>) -> HybridRetriever {
    HybridRetriever::new(
        Arc::clone(double) as Arc<dyn Store>,
        Arc::clone(real) as Arc<dyn IndexStore>,
    )
}

#[test]
fn t05b_mid_reinforce_failure_fails_retrieve_closed() {
    let real = fx::store();
    for id in ["A", "B", "C"] {
        fx::put_lexical(
            &real,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let double = faulty(&real, 2);
    let retriever = retriever(&real, &double);
    let err = retriever
        .retrieve(&RetrieveRequest::new("bank", "alpha"))
        .expect_err("second reinforce failure must fail retrieve closed");
    assert_eq!(err.code, ErrorCode::CoactivationHandoffFailed);
    assert_eq!(
        double.reinforce_calls.load(Ordering::SeqCst),
        2,
        "must stop at the failing pair, no silent skip"
    );
}

#[test]
fn t04c_successful_retrieve_emits_reinforce_telemetry() {
    let real = fx::store();
    for id in ["A", "B"] {
        fx::put_lexical(
            &real,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let double = faulty(&real, 0);
    let retriever = retriever(&real, &double);
    let out = retriever
        .retrieve(&RetrieveRequest::new("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(out.pairs_reinforced, 1);
    assert_eq!(
        double.audits.load(Ordering::SeqCst),
        1,
        "one PII-safe reinforce audit per wave"
    );
}

impl clio_store::TripleAssertion for FaultyStore {}
