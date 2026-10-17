// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance trigger's store-read proof:
//! a counting store double shows that an idle tick reads only the tick banks it
//! selects, never the store's full historical bank list.
//!
//! ## Owns
//! - The delegating `CountingStore` and its recorded bank reads
//!
//! ## Does not own
//! - The active-bank selection (`maintenance_trigger_activity`)
//! - The pressure read and its sources (`maintenance_trigger`)
//! - The fixture state (`maintenance_trigger_tests`)
//!
//! ## Boundary
//!
//! Changes to which banks a tick reads should normally require changes here.
//! Changes to the per-bank decision should not.
//!
//! Test-only. Never compiled into production builds.

use super::maintenance_trigger_tests::{BANK, base, item_in, state};
use super::*;
use clio_store::{
    AssocEdgeRecord, ClaimedSourceState, CorrectionOutcome, CorrectionSpec, DistillIntent,
    DistilledUnitRecord, EmbeddingSpace, ItemAuditTrail, ItemIdentity, ItemInspectFilter,
    ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery, Store, VectorSlot,
};
use clio_types::{
    AmError, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery, TripleRecord,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Every delegated call reaches the same store the fixture opened, and each
/// `list_distilled_units` bank read is recorded, so a tick's bank reads are
/// countable against a store seeded with many unresolved banks.
struct CountingStore {
    inner: Arc<dyn Store>,
    reads: Mutex<Vec<String>>,
}

impl CountingStore {
    fn reads(&self) -> Vec<String> {
        self.reads.lock().expect("spy reads mutex poisoned").clone()
    }
}

/// Forward one call to the inner store, so only the recorded read set differs
/// from the real store's behavior.
macro_rules! delegate_every_call {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for CountingStore {
    delegate_every_call! {
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

impl clio_store::DistilledUnitStore for CountingStore {
    fn list_distilled_units(&self, bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError> {
        self.reads
            .lock()
            .expect("spy reads mutex poisoned")
            .push(bank_id.to_owned());
        self.inner.list_distilled_units(bank_id)
    }

    delegate_every_call! {
        revise_distilled_unit(record: &DistilledUnitRecord, new_item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_distilled_unit(record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        update_distilled_unit_if_unchanged(expected: &DistilledUnitRecord, updated: &DistilledUnitRecord, actor: &str) -> Result<bool, AmError>;
        distilled_unit_claim_state(bank_id: &str, claim_ids: &[String]) -> Result<Vec<ClaimedSourceState>, AmError>;
        write_distill_intent(bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;
        complete_distill_intent(bank_id: &str, hub_id: &str, revision_item_id: &str) -> Result<bool, AmError>;
        get_distill_intent(bank_id: &str, hub_id: &str) -> Result<Option<DistillIntent>, AmError>;
    }
}

impl clio_store::TripleAssertion for CountingStore {}

/// Two idle ticks read exactly the banks the tick selects, once per bank: the
/// unresolved banks seeded in the store are never read, so the tick issues a
/// bounded number of bank reads and never a full-store scan.
#[test]
fn an_idle_tick_reads_only_the_tick_banks_not_the_stored_bank_list() {
    let fixture = state();
    // Seed many banks this runtime never resolved. A tick that listed the
    // store's banks would read every one of them.
    for i in 0..32 {
        let bank = format!("spy-bank-{i}");
        fixture
            .store
            .create_memory_item(&item_in(&format!("spy-{i}"), &bank), "agent")
            .expect("seed idle bank");
    }
    let mut scoped = (*fixture).clone();
    let spy = Arc::new(CountingStore {
        inner: Arc::clone(&scoped.store),
        reads: Mutex::new(Vec::new()),
    });
    scoped.store = spy.clone();

    scoped.evaluate_maintenance_trigger(base());
    scoped.evaluate_maintenance_trigger(base() + Duration::from_secs(5));

    let mut tick_banks = scoped
        .maintenance_trigger
        .tick_banks(&scoped.default_bank, scoped.shared_enabled);
    tick_banks.sort_unstable();
    let mut expected_banks = vec![BANK.to_owned(), scoped.default_bank];
    expected_banks.sort_unstable();
    assert_eq!(
        tick_banks, expected_banks,
        "the fixture's tick set is the resolved bank and the default bank"
    );

    let mut reads = spy.reads();
    reads.sort_unstable();
    let mut expected: Vec<String> = tick_banks
        .iter()
        .chain(tick_banks.iter())
        .cloned()
        .collect();
    expected.sort_unstable();
    assert_eq!(
        reads, expected,
        "two idle ticks must read each tick bank once per tick and no other bank"
    );
    for bank in &reads {
        assert!(
            !bank.starts_with("spy-bank-"),
            "the tick read an unresolved store bank `{bank}`"
        );
    }
}
