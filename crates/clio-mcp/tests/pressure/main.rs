// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pressure-witness integration-test target
//! root: it declares the target's modules, includes the shared read fixture
//! state, and builds the counting-double installation plus the fixed fixtures
//! the zero-write scenarios call through.
//!
//! ## Owns
//! - The `pressure` Cargo test target and its module declarations
//! - The seeded state, the double installation over the handler's
//!   `Arc<dyn Store>` handle and every view trait object that carries store
//!   writers (`state.sync` and `state.kms` are not wrapped; `with_double`
//!   records why), and the dummy writer arguments
//!
//! ## Does not own
//! - The counting double (`store_double`) or its scenarios (`zero_write`)
//! - The shared read fixture state (`mcp_read_support`)
//!
//! ## Boundary
//!
//! Changes to the shared witness fixtures should normally require changes here.
//! Changes to the counting double itself should not.
//!
//! Keep this module limited to the target root and its shared fixtures.

#[path = "../mcp_read_support/mod.rs"]
mod support;

mod store_double;
mod supersession_zero_write;
mod zero_write;

/// Forward one `reads` block untouched to `self.inner` and count every method
/// of the `writes` block into the shared view-writes counter before it
/// delegates. Modeled on the `forward_store!` macro in `store_double`.
macro_rules! view_store {
    (reads { $($rname:ident($($rarg:ident: $rty:ty),* $(,)?) -> $rret:ty;)* }
     writes { $($wname:ident($($warg:ident: $wty:ty),* $(,)?) -> $wret:ty;)* }) => {
        $(fn $rname(&self $(, $rarg: $rty)*) -> $rret {
            self.inner.$rname($($rarg),*)
        })*
        $(fn $wname(&self $(, $warg: $wty)*) -> $wret {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner.$wname($($warg),*)
        })*
    };
}

use crate::store_double::CountingStore;
use clio_compliance::BeliefContextCorrector;
use clio_mcp::index_drain::{INDEX_MAX_BATCH, IndexDrain};
use clio_mcp::runtime::McpState;
use clio_mcp::write_tools::dispatch;
use clio_retrieve::{AssociationEngine, HybridRetriever};
use clio_store::{
    AssocEdgeRecord, AssocKind, AssocPolicy, BatchStore, BatchWrite, BeliefStore, ComplianceStore,
    ComplianceTombstoneView, ContinuousStore, CorrectionSpec, DEFAULT_EMBEDDING_MODEL,
    DistilledUnitRecord, EMBEDDING_DIMS, EmbeddingSpace, HistoryStore, HygieneLogEntry,
    HygieneStore, IndexCoverage, IndexJob, IndexStore, IntegritySignals, ItemIdentity, KnnHit,
    MemTreeNodeRecord, OpsStore, PersonaStore, STALE_SOURCE_CHANGED, SearchHit, Store, StoreCounts,
    SubjectEraseOutcome, SubjectEraseSpec, VectorSlot,
};
use clio_types::{
    AmError, AttributeSchema, BeliefObject, ConfidenceEntry, ContinuousPoint, ContinuousState,
    EpistemicKind, FailureRecord, ItemKind, MemoryItem, PersonaDocument, PersonaPreference,
    PersonaStableEntry, SearchDomain, SemanticCategory, TaskRecord, TimeAxis, TripleRecord,
};
use clio_write::{HUB_DISTILL_MARKER, HubDistillEngine};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Fixed timestamp for every synthetic writer argument.
pub const FIXTURE_TS: &str = "2026-01-01T00:00:00Z";
/// Bank used by the writer-proof dummy arguments.
pub const FIXTURE_BANK: &str = "pressure-witness-dummy";

/// Seed two memory items plus one stored-stale distilled unit through the real
/// store, before any double is installed.
///
/// The shared `support::seed` helper is not reused here: its gated `store`
/// writes dispatch detached maintenance hooks that would move the dirty set
/// this status read reports. Direct repository writes keep every input fixed.
///
/// # Panics
///
/// Panics if a seed write to the backing store fails.
#[must_use]
pub fn seeded_state() -> Arc<McpState> {
    let state = support::fresh_state();
    for (id, gist) in [
        ("pw-item-1", "plain note one"),
        ("pw-item-2", "plain note two"),
    ] {
        state
            .store
            .create_memory_item(&fixture_item(support::BANK, id, gist), "agent")
            .expect("seed item");
    }
    state
        .store
        .commit_distilled_unit(
            &fixture_unit_item(support::BANK, "pw-unit", "distilled summary"),
            &fixture_stale_record(support::BANK, "pw-unit"),
            "agent",
        )
        .expect("seed stale unit");
    state
}

/// A forwarding double over one backend view handle (`OpsStore`, `IndexStore`,
/// persona, belief, history, continuous, compliance, hygiene, the
/// belief-context corrector, and the batch view). Reads forward untouched;
/// each mutating method counts into a shared counter first, so the witness
/// sees a write through any wrapped handle, not only through `Store`.
struct ViewDouble<T: ?Sized> {
    inner: Arc<T>,
    writes: Arc<AtomicUsize>,
}

/// Wrap one view handle in a [`ViewDouble`] sharing `writes`.
#[must_use]
fn view_double<T: ?Sized>(inner: &Arc<T>, writes: &Arc<AtomicUsize>) -> Arc<ViewDouble<T>> {
    Arc::new(ViewDouble {
        inner: Arc::clone(inner),
        writes: Arc::clone(writes),
    })
}

/// Install the counting double on every store-backed handle the pressure read
/// can reach: the handler's `Arc<dyn Store>` and every view trait object that
/// carries store writers (ops, index, persona, belief, history, continuous,
/// compliance, hygiene, the belief-context corrector, and the batch view),
/// plus the association engine, the hub eligibility scan, the index drain,
/// and a hybrid retriever rebuilt over the double with a plain
/// `HybridRetriever::new`, so its ranking environment, association policy,
/// and embedder differ from a freshly opened runtime's retriever. The
/// read-only diagnostic twin is rebuilt over the same double with
/// reinforcement disabled, matching the production twin's one difference.
///
/// Two handles the state carries stay unwrapped, with the reasons. `sync` is
/// an `Arc<dyn SyncStore>`; the runtime installs it as `store.clone()`
/// (`runtime_open.rs`), so it is the same store object already wrapped behind
/// `state.store`. The only read dispatch is `maintenance_status`, which reads
/// `state.maint`, the index drain, and the pressure block over `state.store`,
/// `state.hub`, and `state.env`; every store-backed field on that path is
/// wrapped, and no pressure scenario reaches a `SyncStore` method. A
/// forwarding double over its 38 required methods would also push this file
/// past the 450-line cap. `kms` is an `Arc<dyn DekProvider>` outside the
/// store surface; the store received its own clone of the same `Arc` at open
/// time, and no scenario calls the provider through `state.kms`, so a write
/// through it is unreachable from the pressure read path.
///
/// # Panics
///
/// Panics if rebuilding the index drain over the counting store fails.
pub fn with_double(state: &Arc<McpState>, double: &Arc<CountingStore>) -> Arc<McpState> {
    let store: Arc<dyn Store> = Arc::clone(double) as Arc<dyn Store>;
    let writes = double.view_writes_counter();
    let mut scoped = state.as_ref().clone();
    scoped.store = Arc::clone(&store);
    let index: Arc<dyn IndexStore> = view_double(&scoped.index, &writes);
    scoped.index = Arc::clone(&index);
    scoped.ops_store = view_double(&scoped.ops_store, &writes);
    scoped.persona = view_double(&scoped.persona, &writes);
    scoped.belief = view_double(&scoped.belief, &writes);
    scoped.belief_corrector = view_double(&scoped.belief_corrector, &writes);
    scoped.history = view_double(&scoped.history, &writes);
    scoped.continuous = view_double(&scoped.continuous, &writes);
    scoped.compliance = view_double(&scoped.compliance, &writes);
    scoped.hygiene = view_double(&scoped.hygiene, &writes);
    scoped.batch = view_double(&scoped.batch, &writes);
    scoped.assoc = AssociationEngine::new(Arc::clone(&store), scoped.assoc.policy());
    scoped.retriever = Arc::new(HybridRetriever::new(Arc::clone(&store), Arc::clone(&index)));
    let mut diagnostic_retriever = HybridRetriever::new(Arc::clone(&store), Arc::clone(&index));
    diagnostic_retriever.set_reinforcement(false);
    scoped.diagnostic_retriever = Arc::new(diagnostic_retriever);
    scoped.hub = Some(Arc::new(HubDistillEngine::new(
        Arc::clone(&store),
        scoped.env.clone(),
        AssocPolicy::from_coactivation(&scoped.env.coactivation),
    )));
    scoped.index_drain = Arc::new(
        IndexDrain::new(store, index, scoped.ops_embedder.clone(), INDEX_MAX_BATCH)
            .expect("rebuild the index drain over the counting store"),
    );
    Arc::new(scoped)
}

/// Dispatch `maintenance_status` with the fixed bank and timestamp.
#[must_use]
pub fn status_call(state: &McpState) -> Value {
    dispatch(
        state,
        "maintenance_status",
        &json!({"bank": support::BANK, "timestamp": support::TS}),
    )
}

/// One valid semantic fact; the writer proof reuses it for every item argument.
#[must_use]
pub fn fixture_item(bank: &str, id: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: Some("fixture".into()),
        snapshot: Some(json!({"entity": id})),
        gist: Some(gist.into()),
        valid_from: FIXTURE_TS.into(),
        valid_until: None,
        tx_from: FIXTURE_TS.into(),
        tx_until: None,
        created_at: FIXTURE_TS.into(),
        updated_at: FIXTURE_TS.into(),
        context: None,
    }
}

/// A distill-shaped item whose `source_ref` carries the marker the record
/// validator requires.
#[must_use]
pub fn fixture_unit_item(bank: &str, id: &str, gist: &str) -> MemoryItem {
    let mut item = fixture_item(bank, id, gist);
    item.source_ref = Some(HUB_DISTILL_MARKER.to_owned());
    item
}

/// A stored stale distilled-unit record: the reason is set, so the pressure
/// read must count it.
#[must_use]
pub fn fixture_stale_record(bank: &str, id: &str) -> DistilledUnitRecord {
    let mut record = DistilledUnitRecord::new(
        bank,
        id,
        "hub",
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        FIXTURE_TS,
    );
    record.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
    record.stale_at = Some(FIXTURE_TS.to_owned());
    record
}

/// Dummy item for the writer proof.
#[must_use]
pub fn dummy_item(id: &str) -> MemoryItem {
    fixture_item(FIXTURE_BANK, id, &format!("gist {id}"))
}

/// Dummy distilled-unit record (current, not stale) for the writer proof.
#[must_use]
pub fn dummy_record(id: &str) -> DistilledUnitRecord {
    DistilledUnitRecord::new(
        FIXTURE_BANK,
        id,
        "hub",
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        FIXTURE_TS,
    )
}

/// Dummy correction spec pointing at a missing prior.
#[must_use]
pub fn dummy_correction(new_item: MemoryItem) -> CorrectionSpec {
    CorrectionSpec {
        bank_id: FIXTURE_BANK.into(),
        prior_item_id: "missing-prior".into(),
        new_item,
        reason: "fixture correction".into(),
        actor: "agent".into(),
        timestamp: FIXTURE_TS.into(),
    }
}

/// Dummy association edge.
#[must_use]
pub fn dummy_edge() -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: "edge-1".into(),
        bank_id: FIXTURE_BANK.into(),
        source_id: "a".into(),
        target_id: "b".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("relates_to".into()),
        weight: 0.5,
        last_updated_at: FIXTURE_TS.into(),
    }
}

/// Dummy `MemTree` node row.
#[must_use]
pub fn dummy_node() -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: "node-1".into(),
        bank_id: FIXTURE_BANK.into(),
        parent_id: None,
        item_id: None,
        depth: 0,
        interval_start: FIXTURE_TS.into(),
        interval_end: FIXTURE_TS.into(),
        child_ids: Vec::new(),
        summary_gist: None,
        dirty: false,
        generation: 0,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: FIXTURE_TS.into(),
        updated_at: FIXTURE_TS.into(),
    }
}

/// Dummy vector slot at the active space width.
#[must_use]
pub fn dummy_slot() -> VectorSlot {
    VectorSlot {
        item_id: "vector-1".into(),
        bank_id: FIXTURE_BANK.into(),
        model_id: DEFAULT_EMBEDDING_MODEL.into(),
        embedding: vec![0.0; EMBEDDING_DIMS],
    }
}

/// Dummy SPO triple carrying its own fact-shaped carrier.
#[must_use]
pub fn dummy_triple() -> TripleRecord {
    TripleRecord {
        id: "triple-1".into(),
        bank_id: FIXTURE_BANK.into(),
        item_id: "w-triple".into(),
        subject: "s".into(),
        predicate: "p".into(),
        object: "o".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: FIXTURE_TS.into(),
        valid_until: None,
        tx_from: FIXTURE_TS.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: FIXTURE_TS.into(),
        updated_at: None,
    }
}

impl OpsStore for ViewDouble<dyn OpsStore> {
    view_store! {
        reads { counts() -> Result<StoreCounts, AmError>; ping() -> Result<(), AmError>; schema_version() -> Result<Option<String>, AmError>; embedding_space() -> Result<EmbeddingSpace, AmError>; bank_ids() -> Result<Vec<String>, AmError>; bank_exists(bank_id: &str) -> Result<bool, AmError>; integrity_signals(bank: Option<&str>) -> Result<IntegritySignals, AmError>; }
        writes { register_bank(bank_id: &str, created_at: &str) -> Result<(), AmError>; drop_orphan_assoc_edges(bank: &str) -> Result<u64, AmError>; }
    }
}

impl IndexStore for ViewDouble<dyn IndexStore> {
    view_store! {
        reads { lexical_search(bank_id: &str, query: &str, domains: &[SearchDomain], limit: usize) -> Result<Vec<SearchHit>, AmError>; dense_search(bank_id: &str, query: &[f32], domains: &[SearchDomain], limit: usize) -> Result<Vec<KnnHit>, AmError>; pending_index_jobs(limit: usize, backoff_cutoff_ms: Option<&str>, max_attempts: u32) -> Result<Vec<IndexJob>, AmError>; index_coverage(bank_id: Option<&str>) -> Result<IndexCoverage, AmError>; indexable_items(bank_id: &str, limit: usize, offset: usize) -> Result<Vec<String>, AmError>; count_indexable_items(bank_id: &str) -> Result<u64, AmError>; indexable_banks() -> Result<Vec<String>, AmError>; }
        writes { put_lexical_doc(bank_id: &str, item_id: &str, text: &str) -> Result<(), AmError>; delete_lexical_doc(bank_id: &str, item_id: &str) -> Result<bool, AmError>; enqueue_index_job(bank_id: &str, item_id: &str) -> Result<bool, AmError>; finish_index_job(bank_id: &str, item_id: &str) -> Result<(), AmError>; fail_index_job(bank_id: &str, item_id: &str, error: &str) -> Result<(), AmError>; cleanup_orphans(bank_id: Option<&str>) -> Result<u64, AmError>; }
    }
}

impl PersonaStore for ViewDouble<dyn PersonaStore> {
    view_store! {
        reads { get_persona_document(bank_id: &str) -> Result<PersonaDocument, AmError>; get_preference(bank_id: &str, key: &str) -> Result<Option<PersonaPreference>, AmError>; stable_history(bank_id: &str, key: &str) -> Result<Vec<PersonaStableEntry>, AmError>; inspect_stable_ciphertext(bank_id: &str, key: &str) -> Result<Option<String>, AmError>; }
        writes { put_stable_entry(entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError>; put_preference_state(pref: &PersonaPreference, actor: &str) -> Result<(), AmError>; }
    }
}

impl BeliefStore for ViewDouble<dyn BeliefStore> {
    view_store! {
        reads { get_belief(bank_id: &str, belief_id: &str) -> Result<Option<BeliefObject>, AmError>; find_belief_by_proposition(bank_id: &str, proposition_key: &str) -> Result<Option<BeliefObject>, AmError>; belief_history(bank_id: &str, belief_id: &str, as_of: Option<&str>) -> Result<Vec<ConfidenceEntry>, AmError>; inspect_belief_ciphertext(bank_id: &str, belief_id: &str) -> Result<Option<String>, AmError>; list_beliefs(bank_id: &str) -> Result<Vec<BeliefObject>, AmError>; }
        writes { create_belief(belief: &BeliefObject, actor: &str) -> Result<(), AmError>; append_confidence(bank_id: &str, belief_id: &str, entry: &ConfidenceEntry, actor: &str) -> Result<(), AmError>; append_confidence_with_context(bank_id: &str, belief_id: &str, entry: &ConfidenceEntry, actor: &str, context: Option<&str>) -> Result<(), AmError>; }
    }
}

impl HistoryStore for ViewDouble<dyn HistoryStore> {
    view_store! {
        reads { get_task(bank_id: &str, task_id: &str) -> Result<Option<TaskRecord>, AmError>; task_history(bank_id: &str, task_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError>; list_tasks(bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError>; get_failure(bank_id: &str, failure_id: &str) -> Result<Option<FailureRecord>, AmError>; failures_for_task_exact(bank_id: &str, task_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError>; list_failures(bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError>; triple_history(bank_id: &str, subject: &str, predicate: &str, limit: usize) -> Result<Vec<TripleRecord>, AmError>; triple_history_at(bank_id: &str, subject: &str, predicate: &str, as_of: &str, axis: TimeAxis, limit: usize) -> Result<Vec<TripleRecord>, AmError>; }
        writes { commit_task_upsert(item: &MemoryItem, draft: &TaskRecord, actor: &str) -> Result<TaskRecord, AmError>; commit_failure_record(item: &MemoryItem, draft: &FailureRecord, actor: &str) -> Result<FailureRecord, AmError>; }
    }
}

impl ContinuousStore for ViewDouble<dyn ContinuousStore> {
    view_store! {
        reads { get_attribute_schema(bank_id: &str, attr_key: &str) -> Result<Option<AttributeSchema>, AmError>; continuous_series(bank_id: &str, attr_key: &str, as_of: Option<&str>, limit: usize) -> Result<Vec<ContinuousPoint>, AmError>; }
        writes { put_attribute_schema(schema: &AttributeSchema, actor: &str) -> Result<(), AmError>; observe_continuous(bank_id: &str, attr_key: &str, value: f64, as_of: &str, actor: &str) -> Result<ContinuousState, AmError>; observe_continuous_with_reason(bank_id: &str, attr_key: &str, value: f64, as_of: &str, actor: &str, reason: Option<&str>) -> Result<ContinuousState, AmError>; }
    }
}

impl ComplianceStore for ViewDouble<dyn ComplianceStore> {
    view_store! {
        reads { list_subject_items(bank_id: &str, subject_id: &str) -> Result<Vec<ItemIdentity>, AmError>; subject_dek_destroyed(subject_id: &str) -> Result<bool, AmError>; read_tombstones(bank_id: &str, request_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError>; list_tombstones(bank_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError>; }
        writes { erase_subject(spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError>; }
    }
}

impl HygieneStore for ViewDouble<dyn HygieneStore> {
    view_store! {
        reads { list_hygiene_logs(bank_id: Option<&str>, limit: usize, offset: usize) -> Result<Vec<HygieneLogEntry>, AmError>; }
        writes { set_hygiene_flag(bank_id: &str, item_id: &str, note: Option<&str>, actor: &str, timestamp: &str) -> Result<ItemIdentity, AmError>; archive_item(bank_id: &str, item_id: &str, archived_at: &str, actor: &str) -> Result<ItemIdentity, AmError>; unarchive_item(bank_id: &str, item_id: &str, actor: &str) -> Result<ItemIdentity, AmError>; insert_hygiene_log(entry: &HygieneLogEntry) -> Result<(), AmError>; }
    }
}

impl BeliefContextCorrector for ViewDouble<dyn BeliefContextCorrector> {
    view_store! {
        reads {}
        writes { correct_context(bank_id: &str, belief_id: &str, new_context: &str, reason: &str, actor: &str) -> Result<(), AmError>; }
    }
}

impl BatchStore for ViewDouble<dyn BatchStore> {
    view_store! {
        reads {}
        writes { apply_batch(writes: &[BatchWrite]) -> Result<(), AmError>; }
    }
}
