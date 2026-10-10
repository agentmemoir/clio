// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the counting delegating `Store` double the
//! pressure-witness tests read through: every mutating trait method counts
//! before it forwards, so a read that reaches a writer is visible as a number.
//!
//! ## Owns
//! - `CountingStore`: all 41 `Store` and 8 `DistilledUnitStore` methods forwarded
//! - The 24-method mutation counter, its named list, and the list fault switch
//! - The writer-name derivation from the `Store`/`DistilledUnitStore` sources
//!
//! ## Does not own
//! - The dummy writer arguments and the view-handle doubles (`main`)
//! - The pressure read itself (`clio-mcp` pressure assembly)
//! - Backend behavior: the wrapped store still decides every result
//!
//! ## Boundary
//!
//! Changes to the `Store` contract should normally require changes here.
//! Changes to pressure assembly should not require changes here.
//!
//! Keep this module focused on witnessing store writes, not on modeling them.

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
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Every `Store`/`DistilledUnitStore` method documented as writing, in trait
/// order. The writer proof asserts its call count equals this list's length, so
/// a call omitted from the test cannot pass as covered, and compares this list
/// against [`derived_writer_names`], so a hand edit that drifts from the trait
/// sources cannot pass either.
pub const WRITER_NAMES: [&str; 23] = [
    "delete_item",
    "create_memory_item",
    "update_memory_item",
    "put_assoc_edge",
    "delete_assoc_edge",
    "reinforce_assoc_edge",
    "record_assoc_audit",
    "record_audit_event",
    "upsert_vector",
    "delete_vector",
    "upsert_subject_key_ref",
    "upsert_memtree_node",
    "commit_triple_add",
    "triple_end",
    "invalidate_item",
    "discard_item",
    "correct_item",
    "commit_distilled_unit",
    "revise_distilled_unit",
    "update_distilled_unit",
    "update_distilled_unit_if_unchanged",
    "write_distill_intent",
    "complete_distill_intent",
];

/// A `Store` that forwards to `inner` and counts every documented writer.
///
/// Counting happens before delegation: a call the inner store rejects still
/// registers, so "zero writes" cannot be satisfied by an erroring store. The
/// same instance also owns the counter shared with the view-handle doubles the
/// target root wraps the other backend views over.
pub struct CountingStore {
    inner: Arc<dyn Store>,
    mutations: AtomicUsize,
    view_writes: Arc<AtomicUsize>,
    fail_list_distilled_units: bool,
}

impl CountingStore {
    /// Forward everything, count writers, no injected fault.
    #[must_use]
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self::with_list_distilled_units_fault(inner, false)
    }

    /// A double whose `list_distilled_units` returns `Internal` when `fail` is
    /// true, so a failing stale-state source can be observed.
    #[must_use]
    pub fn with_list_distilled_units_fault(inner: Arc<dyn Store>, fail: bool) -> Self {
        Self {
            inner,
            mutations: AtomicUsize::new(0),
            view_writes: Arc::new(AtomicUsize::new(0)),
            fail_list_distilled_units: fail,
        }
    }

    /// Mutating trait methods reached through this double so far.
    #[must_use]
    pub fn mutations(&self) -> usize {
        self.mutations.load(Ordering::SeqCst)
    }

    /// Writer calls that arrived through the view-handle doubles sharing this
    /// counter, so a write behind any wrapped backend view is visible.
    #[must_use]
    pub fn view_writes(&self) -> usize {
        self.view_writes.load(Ordering::SeqCst)
    }

    /// The shared counter the view-handle doubles increment.
    #[must_use]
    pub fn view_writes_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.view_writes)
    }

    /// Count first, delegate second.
    fn count_mutation(&self) {
        self.mutations.fetch_add(1, Ordering::SeqCst);
    }
}

/// Forward one block of read-only trait methods to `self.inner`, one entry per
/// method. Used only by the store doubles in this file.
macro_rules! forward_store {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.inner.$name($($arg),*)
            }
        )*
    };
}

/// The 19 mutating `Store` trait methods and the 5 mutating
/// `DistilledUnitStore` methods; four do not begin with a write verb
/// (`reinforce_assoc_edge`, `invalidate_item`, `discard_item`, `correct_item`).
/// Each counts, then delegates.
macro_rules! count_store {
    ($($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.count_mutation();
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for CountingStore {
    count_store! {
        delete_item(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        create_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_memory_item(item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        put_assoc_edge(edge: &AssocEdgeRecord) -> Result<(), AmError>;
        delete_assoc_edge(bank_id: &str, edge_id: &str) -> Result<bool, AmError>;
        reinforce_assoc_edge(bank_id: &str, a: &str, b: &str, now_millis: i64, eta: f64) -> Result<AssocEdgeRecord, AmError>;
        record_assoc_audit(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        record_audit_event(bank_id: &str, operation: &str, item_id: Option<&str>, actor: &str, detail: &Value) -> Result<(), AmError>;
        upsert_vector(slot: &VectorSlot) -> Result<(), AmError>;
        delete_vector(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        upsert_subject_key_ref(subject_id: &str, dek_ref: &str, status: &str) -> Result<(), AmError>;
        upsert_memtree_node(node: &MemTreeNodeRecord) -> Result<(), AmError>;
        commit_triple_add(item: &MemoryItem, edge: &TripleRecord, supersede: bool, actor: &str) -> Result<TripleAddOutcome, AmError>;
        triple_end(bank_id: &str, subject: &str, predicate: &str, object: Option<&str>, valid_until: &str, tx_until: &str) -> Result<TripleEndOutcome, AmError>;
        invalidate_item(bank_id: &str, item_id: &str, valid_until: &str, tx_until: &str, replacement_id: Option<&str>, actor: &str) -> Result<TripleEndOutcome, AmError>;
        discard_item(bank_id: &str, item_id: &str, reason: &str, discarded_at: &str, actor: &str) -> Result<ItemIdentity, AmError>;
        correct_item(spec: &CorrectionSpec) -> Result<CorrectionOutcome, AmError>;
        commit_distilled_unit(item: &MemoryItem, record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
    }

    forward_store! {
        get_memory_item(bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;
        get_snapshot(bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;
        get_gist(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        get_assoc_edge(bank_id: &str, edge_id: &str) -> Result<Option<AssocEdgeRecord>, AmError>;
        list_assoc_edges(bank_id: &str, item_id: Option<&str>) -> Result<Vec<AssocEdgeRecord>, AmError>;
        get_item_by_subject(bank_id: &str, subject_id: &str, as_of: Option<&str>) -> Result<Option<MemoryItem>, AmError>;
        knn(bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;
        embedding_space() -> Result<EmbeddingSpace, AmError>;
        inspect_content_ciphertext(bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;
        get_memtree_node(bank_id: &str, node_id: &str) -> Result<Option<MemTreeNodeRecord>, AmError>;
        query_memtree_nodes(query: &MemTreeQuery) -> Result<Vec<MemTreeNodeRecord>, AmError>;
        get_triple(bank_id: &str, edge_id: &str) -> Result<Option<TripleRecord>, AmError>;
        query_triples(query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError>;
        get_item_identity(bank_id: &str, item_id: &str) -> Result<Option<ItemIdentity>, AmError>;
        item_is_archived(bank_id: &str, item_id: &str) -> Result<bool, AmError>;
        item_ids_in_window(bank_id: &str, axis: TimeAxis, start: &str, end: &str, limit: usize) -> Result<Vec<String>, AmError>;
        inspect_items(filter: &ItemInspectFilter) -> Result<Vec<ItemInspectRow>, AmError>;
        count_items(filter: &ItemInspectFilter) -> Result<usize, AmError>;
        audit_trail(bank_id: &str, item_id: &str) -> Result<ItemAuditTrail, AmError>;
        list_triples(bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
        get_distilled_unit(bank_id: &str, item_id: &str) -> Result<Option<DistilledUnitRecord>, AmError>;
    }
}

impl clio_store::DistilledUnitStore for CountingStore {
    fn list_distilled_units(&self, bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError> {
        if self.fail_list_distilled_units {
            return Err(AmError::new(ErrorCode::Internal, "synthetic list failure"));
        }
        self.inner.list_distilled_units(bank_id)
    }

    count_store! {
        revise_distilled_unit(record: &DistilledUnitRecord, new_item: &MemoryItem, actor: &str) -> Result<(), AmError>;
        update_distilled_unit(record: &DistilledUnitRecord, actor: &str) -> Result<(), AmError>;
        update_distilled_unit_if_unchanged(expected: &DistilledUnitRecord, updated: &DistilledUnitRecord, actor: &str) -> Result<bool, AmError>;
        write_distill_intent(bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;
        complete_distill_intent(bank_id: &str, hub_id: &str, revision_item_id: &str) -> Result<bool, AmError>;
    }

    forward_store! {
        distilled_unit_claim_state(bank_id: &str, claim_ids: &[String]) -> Result<Vec<ClaimedSourceState>, AmError>;
        get_distill_intent(bank_id: &str, hub_id: &str) -> Result<Option<DistillIntent>, AmError>;
    }
}

// Writer-name derivation. The trait doc comments carry no explicit writer
// marker: a mutating method's first doc line opens with a mutation lead
// ("Legacy", "Ungated", "Insert", "Delete", "Upsert", "Append", "Persist",
// "Close", "Write", "Atomic", "Logged", "Record") and a reading method opens
// with a read/return lead ("Read", "List", "Query", "Latest", "Every", "True",
// "Candidate", "Metadata-only", "Total", "Reconstruct", "Exact", "The",
// "Pattern", "Bank-scoped", "Authoritative", "Non-authoritative"). One method
// (`update_distilled_unit_if_unchanged`) opens with an intra-doc link
// (`[`Self::update_distilled_unit`]`) and inherits that method's class.

/// Mutation lead words the trait doc comments open a mutating method with.
const WRITER_DOC_LEADS: [&str; 12] = [
    "Legacy", "Ungated", "Insert", "Delete", "Upsert", "Append", "Persist", "Close", "Write",
    "Atomic", "Logged", "Record",
];

/// Read/return lead words the same doc comments open a reading method with.
const READ_DOC_LEADS: [&str; 16] = [
    "Read",
    "List",
    "Query",
    "Latest",
    "Every",
    "True",
    "Candidate",
    "Metadata-only",
    "Total",
    "Reconstruct",
    "Exact",
    "The",
    "Pattern",
    "Bank-scoped",
    "Authoritative",
    "Non-authoritative",
];

/// `(method name, first doc line)` for every method of the one trait in `path`.
fn trait_method_docs(path: &Path) -> Vec<(String, String)> {
    let src = std::fs::read_to_string(path).expect("trait source readable");
    let mut methods = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    let mut depth = 0usize;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(doc) = trimmed.strip_prefix("///") {
            if depth > 0 {
                docs.push(doc.trim().to_owned());
            }
            continue;
        }
        if depth > 0 && trimmed.starts_with("fn ") {
            let name = trimmed["fn ".len()..]
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default()
                .to_owned();
            let first = docs.first().map_or(String::new(), String::clone);
            methods.push((name, first));
        }
        docs.clear();
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
    }
    methods
}

/// Classify one method's first doc line against the two lead sets: `Some(true)`
/// writer, `Some(false)` read, `None` unclassified. A doc opening with an
/// intra-doc link inherits the linked method's class.
fn classify_doc(doc: &str, methods: &[(String, String)], depth: usize) -> Option<bool> {
    if let Some(rest) = doc.strip_prefix("[`Self::") {
        let target = rest.split('`').next()?;
        let (_, target_doc) = methods.iter().find(|(name, _)| name == target)?;
        return if depth > 0 {
            None
        } else {
            classify_doc(target_doc, methods, 1)
        };
    }
    let first = doc.split_whitespace().next()?;
    if WRITER_DOC_LEADS.iter().any(|lead| first.starts_with(lead)) {
        return Some(true);
    }
    if READ_DOC_LEADS.iter().any(|lead| first.starts_with(lead)) {
        return Some(false);
    }
    None
}

/// Writer names derived from the `Store` and `DistilledUnitStore` trait
/// sources beside this workspace crate. The writer proof compares
/// [`WRITER_NAMES`] against this set, so the hand list cannot drift from the
/// trait without failing.
///
/// # Panics
///
/// Panics when a trait method's doc comment opens with neither lead set: the
/// derivation refuses to guess, so a new method must be classified by its doc.
#[must_use]
pub fn derived_writer_names() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clio-store/src");
    let mut methods = trait_method_docs(&src.join("store.rs"));
    methods.extend(trait_method_docs(&src.join("distilled_unit_store.rs")));
    let mut writers = Vec::new();
    let mut unclassified = Vec::new();
    for (name, doc) in &methods {
        match classify_doc(doc, &methods, 0) {
            Some(true) => writers.push(name.clone()),
            Some(false) => {}
            None => unclassified.push(format!("{name}: {doc}")),
        }
    }
    assert!(
        unclassified.is_empty(),
        "trait doc comments the writer derivation cannot classify: {unclassified:?}"
    );
    writers.sort_unstable();
    writers
}

impl clio_store::TripleAssertion for CountingStore {}
