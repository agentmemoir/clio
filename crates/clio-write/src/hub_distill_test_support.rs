// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the crash-seam store double the hub-distill
//! crash tests arm: it forwards every call to the real store and fails one
//! distilled-unit write or one claim completion once, on demand.
//!
//! ## Owns
//! - The one-shot armed flags and fired counters behind them
//! - The failing `commit_distilled_unit` override
//! - The failing `complete_distill_intent` override
//!
//! ## Does not own
//! - The wave envelope the tests drive
//! - The claim lifecycle the wave owns
//!
//! ## Boundary
//!
//! Changes to how a test faults the unit write or the claim completion should
//! normally require changes here. Changes to the wave or the store contract
//! should not require changes here.
//!
//! Keep this module focused on faulting one write once.
//!
//! Test-only. Never compiled into production builds.

use clio_store::{
    AssocEdgeRecord, ClaimedSourceState, CorrectionOutcome, CorrectionSpec, DistillIntent,
    DistilledUnitRecord, EmbeddingSpace, ItemAuditTrail, ItemIdentity, ItemInspectFilter,
    ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery, Store, VectorSlot,
};
use clio_types::{
    AmError, ErrorCode, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery,
    TripleRecord,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Forward every trait method to the wrapped store, so the seam adds exactly
/// one behavior: the one-shot unit-write fault below.
macro_rules! forward_store {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.inner.$name($($arg),*)
            }
        )*
    };
}

/// What the seam remembers: whether each fault is armed, and how many times
/// each fired, so a test can prove the fault it reasons about actually
/// happened.
struct SeamState {
    /// Set by `arm`, cleared when the unit-write fault fires. Never set by
    /// default.
    armed: bool,
    /// How many times the unit-write fault fired. One-shot, so never above one.
    fired: usize,
    /// Set by `arm_claim_completion`, cleared when the claim-completion fault
    /// fires. Never set by default.
    claim_armed: bool,
    /// How many times the claim-completion fault fired. One-shot, so never
    /// above one.
    claim_fired: usize,
}

/// A store double that forwards every call to the real store and fails one
/// distilled-unit write or one claim completion once, when armed.
///
/// Arm it explicitly per test with [`Self::arm`] or
/// [`Self::arm_claim_completion`]. The first matching call after arming fails
/// and disarms that fault; every later call passes through. The two faults are
/// independent, so a unit-write arming never touches the claim.
pub(crate) struct CrashSeam {
    inner: Arc<dyn Store>,
    state: Mutex<SeamState>,
}

impl CrashSeam {
    /// Wrap a store. The seam starts disarmed: no test faults anything until
    /// it arms the seam itself.
    pub(crate) fn new(inner: Arc<dyn Store>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(SeamState {
                armed: false,
                fired: 0,
                claim_armed: false,
                claim_fired: 0,
            }),
        })
    }

    /// Arm the one-shot fault: the next distilled-unit write fails.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    pub(crate) fn arm(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .armed = true;
    }

    /// How many times the unit-write fault fired.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    pub(crate) fn fired(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .fired
    }

    /// Arm the one-shot fault: the next claim completion fails, so the wave
    /// stops in the window after the unit lands but before the stamp.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    pub(crate) fn arm_claim_completion(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .claim_armed = true;
    }

    /// How many times the claim-completion fault fired.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    pub(crate) fn claim_fired(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .claim_fired
    }
}

impl Store for CrashSeam {
    forward_store! {
        delete_item(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        create_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        get_memory_item(bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;
        get_snapshot(bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;
        get_gist(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        put_assoc_edge(edge: &AssocEdgeRecord) -> Result<(), AmError>;
        get_assoc_edge(
            bank_id: &str,
            edge_id: &str,
        ) -> Result<Option<AssocEdgeRecord>, AmError>;
        delete_assoc_edge(bank_id: &str, edge_id: &str) -> Result<bool, AmError>;
        list_assoc_edges(
            bank_id: &str,
            item_id: Option<&str>,
        ) -> Result<Vec<AssocEdgeRecord>, AmError>;
        reinforce_assoc_edge(
            bank_id: &str,
            a: &str,
            b: &str,
            now_millis: i64,
            eta: f64,
        ) -> Result<AssocEdgeRecord, AmError>;
        record_assoc_audit(
            bank_id: &str,
            operation: &str,
            item_id: Option<&str>,
            actor: &str,
            detail: &Value,
        ) -> Result<(), AmError>;
        record_audit_event(
            bank_id: &str,
            operation: &str,
            item_id: Option<&str>,
            actor: &str,
            detail: &Value,
        ) -> Result<(), AmError>;
        get_item_by_subject(
            subject_id: &str,
            bank_id: &str,
            as_of: Option<&str>,
        ) -> Result<Option<MemoryItem>, AmError>;
        upsert_vector(slot: &VectorSlot) -> Result<(), AmError>;
        delete_vector(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        knn(bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;
        embedding_space() -> Result<EmbeddingSpace, AmError>;
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
        item_is_archived(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        item_ids_in_window(
            bank_id: &str,
            axis: TimeAxis,
            start: &str,
            end: &str,
            limit: usize,
        ) -> Result<Vec<String>, AmError>;
        inspect_items(
            filter: &ItemInspectFilter,
        ) -> Result<Vec<ItemInspectRow>, AmError>;
        count_items(filter: &ItemInspectFilter) -> Result<usize, AmError>;
        correct_item(spec: &CorrectionSpec) -> Result<CorrectionOutcome, AmError>;
        audit_trail(bank_id: &str, item_id: &str) -> Result<ItemAuditTrail, AmError>;
        list_triples(bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
        get_distilled_unit(
            bank_id: &str,
            item_id: &str,
        ) -> Result<Option<DistilledUnitRecord>, AmError>;
    }

    /// The unit-write fault: when armed, fail once and disarm, so the
    /// wave sees a storage error exactly where a crash would have struck.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn commit_distilled_unit(
        &self,
        item: &MemoryItem,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError> {
        let fire = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if state.armed {
                state.armed = false;
                state.fired += 1;
                true
            } else {
                false
            }
        };
        if fire {
            return Err(AmError::new(
                ErrorCode::Internal,
                "crash seam refused the distilled-unit write",
            ));
        }
        self.inner.commit_distilled_unit(item, record, actor)
    }
}

impl clio_store::DistilledUnitStore for CrashSeam {
    forward_store! {
        list_distilled_units(bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError>;
        revise_distilled_unit(
            record: &DistilledUnitRecord,
            new_item: &MemoryItem,
            actor: &str,
        ) -> Result<(), AmError>;
        update_distilled_unit(
            record: &DistilledUnitRecord,
            actor: &str,
        ) -> Result<(), AmError>;
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
        get_distill_intent(
            bank_id: &str,
            hub_id: &str,
        ) -> Result<Option<DistillIntent>, AmError>;
    }

    /// The seam's second fault: when armed, fail the one claim completion and
    /// disarm, so the wave sees a storage error in the window after the unit
    /// lands but before the claim is stamped complete.
    /// The explicit closure keeps the poisoned mutex usable.
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn complete_distill_intent(
        &self,
        bank_id: &str,
        hub_id: &str,
        revision_item_id: &str,
    ) -> Result<bool, AmError> {
        let fire = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if state.claim_armed {
                state.claim_armed = false;
                state.claim_fired += 1;
                true
            } else {
                false
            }
        };
        if fire {
            return Err(AmError::new(
                ErrorCode::Internal,
                "crash seam refused the claim completion",
            ));
        }
        self.inner
            .complete_distill_intent(bank_id, hub_id, revision_item_id)
    }
}
