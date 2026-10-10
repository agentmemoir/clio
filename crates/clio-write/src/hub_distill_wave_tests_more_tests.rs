// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that watch a hub wave from outside
//! it: the durable completion row the wave asks the store to write, and the
//! per-bank running count it reports while a wave runs and after every exit
//! path.
//!
//! ## Owns
//! - Test-only coverage of the wave completion row a wave leaves in the store
//! - Test-only coverage of `hub_distill_in_flight` on a running wave and after a
//!   wave that errors
//! - The delegating store double and the blocking admission sink these tests use
//!
//! ## Does not own
//! - The wave's hub budget arithmetic (`hub_distill_wave_tests`)
//! - The wave envelope itself (`hub_distill_wave`)
//! - Hub seeding and engine fixtures (`hub_distill_tests`)
//!
//! ## Boundary
//!
//! Changes to what a finished wave records, or to how a running wave is counted,
//! should normally require changes here. Changes to how many hubs a wave may
//! distill should not require changes here.
//!
//! Keep this module focused on what a running or finished wave reports.

use super::super::super::HubDistillEngine;
use super::super::super::hub_distill_tests::policy;
use super::{ctx, engine, three_hubs};
use clio_admission::gated::{AdmissionEvent, EventSink};
use clio_config::ranking::RankingEnv;
use clio_config::retention::ResolvedRetention;
use clio_store::{
    AssocEdgeRecord, ClaimedSourceState, CorrectionOutcome, CorrectionSpec, DistillIntent,
    DistilledUnitRecord, EmbeddingSpace, ItemAuditTrail, ItemIdentity, ItemInspectFilter,
    ItemInspectRow, KnnHit, MemTreeNodeRecord, MemTreeQuery, SqliteStore, Store, VectorSlot,
};
use clio_types::{
    AmError, ErrorCode, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery,
    TripleRecord,
};
use serde_json::Value;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The ceiling on every wait a wave test makes, so a stuck wave fails the test
/// instead of hanging it.
const WAVE_WAIT: Duration = Duration::from_secs(30);

/// Forward every trait method to the wrapped store. The delegating doubles in
/// other crates of this workspace use the same shape, so the trait list is
/// written once here rather than by hand per method.
macro_rules! forward_store {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.inner.$name($($arg),*)
            }
        )*
    };
}

/// A store double that forwards every call to the real store, records the
/// assoc-audit rows a wave asks it to write, and can fail its edge reads.
///
/// The audit row is read back here after the wave returns, so the assertion is
/// on the record the store was asked to persist rather than on the wave's own
/// return value. The store exposes no read for a bank-wide audit row that
/// carries no item id, and the wave's completion row carries none, so this
/// double is the observation point; every row it records is also forwarded to
/// the real store, which persists it.
struct WaveStore {
    inner: Arc<SqliteStore>,
    fail_edge_reads: bool,
    assoc_audits: Mutex<Vec<(String, Value)>>,
}

impl WaveStore {
    /// A double that records assoc-audit rows and reads through, or one whose
    /// edge reads fail so the wave it serves errors while holding its claim.
    fn new(inner: Arc<SqliteStore>, fail_edge_reads: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fail_edge_reads,
            assoc_audits: Mutex::new(Vec::new()),
        })
    }

    /// The detail of every `hub_distill_complete` row this store was asked to
    /// write, in write order.
    fn complete_audits(&self) -> Vec<Value> {
        self.assoc_audits
            .lock()
            .expect("assoc audit log poisoned")
            .iter()
            .filter(|(operation, _)| operation == "hub_distill_complete")
            .map(|(_, detail)| detail.clone())
            .collect()
    }
}

impl Store for WaveStore {
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
        reinforce_assoc_edge(
            bank_id: &str,
            a: &str,
            b: &str,
            now_millis: i64,
            eta: f64,
        ) -> Result<AssocEdgeRecord, AmError>;
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

    fn list_assoc_edges(
        &self,
        bank_id: &str,
        item_id: Option<&str>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError> {
        if self.fail_edge_reads {
            return Err(AmError::new(ErrorCode::Internal, "edge read failed"));
        }
        self.inner.list_assoc_edges(bank_id, item_id)
    }

    fn record_assoc_audit(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.assoc_audits
            .lock()
            .expect("assoc audit log poisoned")
            .push((operation.to_owned(), detail.clone()));
        self.inner
            .record_assoc_audit(bank_id, operation, item_id, actor, detail)
    }
}

impl clio_store::DistilledUnitStore for WaveStore {
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

/// An admission sink that parks the wave inside its own run: it reports that the
/// gate was reached, then waits for the test to release it. Synchronizing on
/// that signal, rather than on a pause, is what makes the running count
/// readable at a known point in the wave.
#[derive(Debug)]
struct BlockingSink {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl EventSink for BlockingSink {
    fn emit(&self, _event: AdmissionEvent) {
        let _ = self.entered.send(());
        self.release
            .lock()
            .expect("release lock poisoned")
            .recv_timeout(WAVE_WAIT)
            .expect("the test released the blocked wave");
    }
}

/// An engine over a delegating double: the recording one for the durable audit
/// row, the failing one for the error path.
fn engine_over(store: Arc<WaveStore>) -> HubDistillEngine {
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 0.0;
    HubDistillEngine::with_clock(store as Arc<dyn Store>, env, policy(2), 0)
}

#[test]
fn a_wave_that_stops_at_the_budget_records_the_left_work_in_the_store() {
    let store = WaveStore::new(three_hubs(), false);
    let engine = engine_over(Arc::clone(&store));
    let wave = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 1)
        .unwrap();
    assert_eq!(wave.hubs_deferred, 2, "{wave:?}");
    let recorded = store.complete_audits();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0]["hubs_deferred"].as_u64(),
        Some(2),
        "{recorded:?}"
    );
    assert_eq!(recorded[0]["distilled"].as_u64(), Some(1), "{recorded:?}");
}

#[test]
fn the_hub_wave_count_returns_to_zero_on_every_exit_path() {
    let store = three_hubs();
    let engine = engine(&store, 0.0, 2);
    assert_eq!(engine.hub_distill_in_flight("bank"), 0, "before any wave");
    engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 1)
        .unwrap();
    assert_eq!(
        engine.hub_distill_in_flight("bank"),
        0,
        "after a wave that finished"
    );

    let failing = engine_over(WaveStore::new(store, true));
    let error = failing
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 3)
        .expect_err("a wave whose first read fails");
    assert_eq!(error.code, ErrorCode::Internal, "{error:?}");
    assert_eq!(
        failing.hub_distill_in_flight("bank"),
        0,
        "a leaked count would wedge this bank out of every later wave"
    );
}

#[test]
fn the_hub_wave_count_is_per_bank_and_nonzero_while_running() {
    let store = three_hubs();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut engine = engine(&store, 0.0, 2);
    engine.set_sink(Arc::new(BlockingSink {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }));
    let engine = Arc::new(engine);
    let wave = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            engine
                .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 1)
                .unwrap()
        })
    };
    entered_rx
        .recv_timeout(WAVE_WAIT)
        .expect("the wave reached its admission gate");
    assert!(
        engine.hub_distill_in_flight("bank") >= 1,
        "a running wave must report itself for its own bank"
    );
    assert_eq!(
        engine.hub_distill_in_flight("other"),
        0,
        "another bank's wave must not be counted here"
    );
    release_tx
        .send(())
        .expect("the wave is still waiting for its release");
    wave.join().expect("the wave thread finished");
    assert_eq!(engine.hub_distill_in_flight("bank"), 0, "after the wave");
    assert_eq!(engine.hub_distill_in_flight("other"), 0, "after the wave");
}
