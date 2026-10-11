// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Shared behavioral persistence contract (both backends).

use crate::audit_model::{CorrectionOutcome, CorrectionSpec, ItemAuditTrail};
use crate::distilled_unit::DistilledUnitRecord;
use crate::embedding_space::EmbeddingSpace;
use crate::model::{
    AssocEdgeRecord, ItemIdentity, ItemInspectFilter, ItemInspectRow, KnnHit, VectorSlot,
};
use crate::triple_assertion::TripleAssertion;
use clio_types::{
    AmError, MemoryItem, TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery, TripleRecord,
};
use serde_json::Value;

/// Backend-agnostic repository for memory items, assoc edges, and vector hooks.
///
/// Memory-item create/update methods are **ungated repository primitives**; public
/// long-term write tools must wrap them with admission gates before commit. This
/// file is near the 450-line cap: the next addition needs a module split, not another reflow.
pub trait Store:
    crate::distilled_unit_store::DistilledUnitStore + TripleAssertion + Send + Sync
{
    /// Delete one item row and its derived rows (vector, lexical, index job,
    /// distilled-unit record, distill intents), scoped by bank, in one
    /// transaction.
    ///
    /// # Errors
    /// Storage failures.
    fn delete_item(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError>;

    /// Ungated create of a typed memory item (dual snapshot/gist).
    ///
    /// # Errors
    /// Validation, duplicate id, crypto, or storage failures.
    fn create_memory_item(&self, item: &MemoryItem, actor: &str) -> Result<(), AmError>;

    /// Ungated update of mutable memory-item fields.
    ///
    /// # Errors
    /// Validation, missing id, crypto, or storage failures.
    fn update_memory_item(&self, item: &MemoryItem, actor: &str) -> Result<(), AmError>;

    /// Read a typed memory item scoped by bank.
    ///
    /// # Errors
    /// Storage or decrypt failures.
    fn get_memory_item(&self, bank_id: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError>;

    /// Authoritative structured snapshot only.
    ///
    /// `Ok(None)` = item missing / wrong bank. `Err(snapshot_missing)` = item exists,
    /// no snapshot half.
    ///
    /// # Errors
    /// Missing snapshot half, storage, or decrypt failures.
    fn get_snapshot(&self, bank_id: &str, item_id: &str) -> Result<Option<Value>, AmError>;

    /// Non-authoritative prose gist only.
    ///
    /// `Ok(None)` = item missing / wrong bank. `Err(gist_missing)` = item exists, no gist.
    ///
    /// # Errors
    /// Missing gist half, storage, or decrypt failures.
    fn get_gist(&self, bank_id: &str, item_id: &str) -> Result<Option<String>, AmError>;

    /// Insert or replace an association edge (graph metadata; plaintext structure).
    ///
    /// # Association edge lifecycle
    /// These rows are graph metadata that reference item ids, not SPO triples:
    /// erasing an item drops the edges whose endpoint it was, and sync payloads
    /// must carry association mutations once multi-host is claimed. Pruning an
    /// edge is an operations removal, never fact invalidation.
    ///
    /// # Errors
    /// Validation or storage failures.
    fn put_assoc_edge(&self, edge: &AssocEdgeRecord) -> Result<(), AmError>;

    /// Read an association edge scoped by bank.
    ///
    /// # Errors
    /// Storage failures.
    fn get_assoc_edge(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<Option<AssocEdgeRecord>, AmError>;

    /// Delete an association edge scoped by bank.
    ///
    /// # Errors
    /// Storage failures.
    fn delete_assoc_edge(&self, bank_id: &str, edge_id: &str) -> Result<bool, AmError>;

    /// List association edges incident to `item_id`, or the whole bank when
    /// `item_id` is `None`. Bank-scoped; `Ok(vec![])` when nothing matches.
    ///
    /// # Errors
    /// Storage failures.
    fn list_assoc_edges(
        &self,
        bank_id: &str,
        item_id: Option<&str>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError>;

    /// Atomically apply one saturating co-activation growth step to the
    /// unordered pair `(a, b)` (create-if-missing), returning the stored edge.
    ///
    /// The increment runs inside a single SQL upsert, so concurrent reinforces
    /// of the same pair cannot lose an update.
    ///
    /// # Errors
    /// Validation or storage failures.
    fn reinforce_assoc_edge(
        &self,
        bank_id: &str,
        a: &str,
        b: &str,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError>;

    /// Append a PII-safe association audit event (ids/weights only, no payloads).
    ///
    /// # Errors
    /// Validation or storage failures.
    fn record_assoc_audit(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError>;

    /// Append a PII-safe generic audit event row (workspace tools: reviewer
    /// attestations, shared-bank telemetry, canonical supersession markers).
    ///
    /// Callers pass ids/counts/labels only — never raw content payloads.
    ///
    /// # Errors
    /// Validation (empty bank/operation/actor) or storage failures.
    fn record_audit_event(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError>;

    /// Latest item row for a subject id, bi-temporally resolved.
    ///
    /// Without `as_of`: the currently active revision (`tx_until IS NULL`).
    /// With `as_of`: the latest revision whose transaction window covers the
    /// point in time. This is the single-slot canonical read primitive.
    ///
    /// # Errors
    /// Storage failures.
    fn get_item_by_subject(
        &self,
        bank_id: &str,
        subject_id: &str,
        as_of: Option<&str>,
    ) -> Result<Option<MemoryItem>, AmError>;

    /// Upsert a dense vector slot (real pgvector / sqlite-vec).
    ///
    /// # Errors
    /// Dimension mismatch or storage failures.
    fn upsert_vector(&self, slot: &VectorSlot) -> Result<(), AmError>;

    /// Delete a dense vector slot scoped by bank.
    ///
    /// # Errors
    /// Storage failures.
    fn delete_vector(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError>;

    /// Exact/brute-force KNN (cosine) over the current belief: the same
    /// visibility rule as the index legs (`tx_until IS NULL`, not discarded,
    /// not archived), so a corrected prior never returns. ANN throughput
    /// parity is not required.
    ///
    /// A query whose length is not the active space width fails closed.
    ///
    /// # Errors
    /// Dimension mismatch or storage failures.
    fn knn(&self, bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError>;

    /// The active embedding space `(model_id, dims)` this store serves.
    ///
    /// # Errors
    /// Storage failures.
    fn embedding_space(&self) -> Result<EmbeddingSpace, AmError>;

    /// Persist / refresh the `subject_keys.dek_ref` row (never stores raw DEK bytes).
    ///
    /// # Errors
    /// Storage failures.
    fn upsert_subject_key_ref(
        &self,
        subject_id: &str,
        dek_ref: &str,
        status: &str,
    ) -> Result<(), AmError>;

    /// Read raw `content_ciphertext` for security inspection tests.
    ///
    /// # Errors
    /// Storage failures.
    fn inspect_content_ciphertext(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<String>, AmError>;

    /// Insert or replace a MemTree node (bank-scoped).
    ///
    /// # Errors
    /// Validation or storage failures.
    fn upsert_memtree_node(
        &self,
        node: &crate::memtree_model::MemTreeNodeRecord,
    ) -> Result<(), AmError>;

    /// Read a MemTree node scoped by bank.
    ///
    /// # Errors
    /// Storage failures.
    fn get_memtree_node(
        &self,
        bank_id: &str,
        node_id: &str,
    ) -> Result<Option<crate::memtree_model::MemTreeNodeRecord>, AmError>;

    /// Query MemTree nodes with filters (bank required).
    ///
    /// # Errors
    /// Validation or storage failures.
    fn query_memtree_nodes(
        &self,
        query: &crate::memtree_model::MemTreeQuery,
    ) -> Result<Vec<crate::memtree_model::MemTreeNodeRecord>, AmError>;

    /// Atomic gated-carrier create + SPO insert with optional subject+predicate supersede.
    ///
    /// Closes prior open edges on **both** time axes (never DELETE). `item` must already
    /// carry an admission score from the gate; this method does not re-score.
    ///
    /// # Errors
    /// Validation, missing refs, illegal coexistence, crypto, or storage failures.
    fn commit_triple_add(
        &self,
        item: &MemoryItem,
        edge: &TripleRecord,
        supersede: bool,
        actor: &str,
    ) -> Result<TripleAddOutcome, AmError>;

    /// Read one SPO edge scoped by bank.
    ///
    /// # Errors
    /// Storage failures.
    fn get_triple(&self, bank_id: &str, edge_id: &str) -> Result<Option<TripleRecord>, AmError>;

    /// Pattern + point-in-time triple query (bank-scoped).
    ///
    /// # Errors
    /// Validation or storage failures.
    fn query_triples(&self, query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError>;

    /// Close matching open SPO edges on both axes (`triple_end`).
    ///
    /// # Errors
    /// Validation or storage failures.
    fn triple_end(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
        valid_until: &str,
        tx_until: &str,
    ) -> Result<TripleEndOutcome, AmError>;

    /// Close both time axes on an item-shaped discrete record (`invalidate`).
    /// Writes the `invalidate` telemetry row inside the same transaction as
    /// the mutation; a `replacement_id` links the prior row forward.
    ///
    /// # Errors
    /// Missing item, triple-id misuse, empty actor, or storage failures.
    fn invalidate_item(
        &self,
        bank_id: &str,
        item_id: &str,
        valid_until: &str,
        tx_until: &str,
        replacement_id: Option<&str>,
        actor: &str,
    ) -> Result<TripleEndOutcome, AmError>;

    /// Bank-scoped item identity columns (no content/DEK access).
    ///
    /// `Ok(None)` = missing / wrong bank. The discard tool uses this to resolve
    /// its dry-run target without decrypting the payload.
    ///
    /// # Errors
    /// Storage failures.
    fn get_item_identity(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<ItemIdentity>, AmError>;

    /// Logged operations removal from active use (`discard`): set `discarded_at` + `discard_reason`
    /// on the item row. Never a supersession and never compliance erasure. Writes the `discard`
    /// telemetry row inside the same transaction as the mutation.
    ///
    /// Unknown / cross-bank `item_id` → `ItemNotFound` (no existence leak). An
    /// already-discarded item is rejected as `InvalidArgument`; content, DEKs,
    /// and supersession edges are untouched.
    ///
    /// # Errors
    /// Missing item, already discarded, empty reason/actor, or storage failures.
    fn discard_item(
        &self,
        bank_id: &str,
        item_id: &str,
        reason: &str,
        discarded_at: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError>;

    /// True when the item carries the hygiene archive marker.
    ///
    /// Archived items are excluded from default retrieve but stay readable by
    /// id; the probe lets retrieval eligibility checks stay content-free.
    ///
    /// # Errors
    /// Storage failures.
    fn item_is_archived(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError>;

    /// Candidate item ids whose interval on `axis` overlaps the half-open window `[start, end)`,
    /// best-first by interval start descending then id ascending, capped at `limit`.
    ///
    /// Membership follows the same half-open interval convention `as_of` uses
    /// (`interval_contains`): the item matches when its interval start is before `end` and its
    /// interval end is absent or after `start`. An open-ended item whose interval started before
    /// the window therefore matches: it was still true inside the window.
    ///
    /// The query's SQL pre-filters four clauses: bank scope, current transaction belief
    /// (`tx_until IS NULL`), not discarded, and not archived. The shared candidate fetch then
    /// applies the request's domains, the `as_of`/`time_axis` point filter, and a re-probe of
    /// the archive marker, so this query is candidates only, never a final answer.
    ///
    /// On `TimeAxis::Transaction` the current-belief clause subsumes the start disjunction: a
    /// candidate needs `tx_from` before the window end and `tx_until` absent, so the window start
    /// bound is inert on this axis. The valid axis keeps the full two-bound overlap. Bounds are
    /// compared as ISO-8601 text, the same convention the store writes.
    ///
    /// Returns ids only: never content, scores, or DEK material.
    ///
    /// # Errors
    /// Empty bank or bounds, `start >= end`, a zero limit, a limit above
    /// the window cap (`MAX_WINDOW_ITEMS`), or storage failures.
    fn item_ids_in_window(
        &self,
        bank_id: &str,
        axis: TimeAxis,
        start: &str,
        end: &str,
        limit: usize,
    ) -> Result<Vec<String>, AmError>;

    /// Metadata-only item listing for the memory inspector.
    ///
    /// Returns ids, taxonomy, scores, validity windows, and refs — never ciphertext, DEKs, or
    /// decrypted content. Bank-scoped and deterministically ordered by `(created_at, id)`;
    /// paginated by the filter's `limit`/`offset` (capped at 200 rows per page).
    ///
    /// # Errors
    /// Empty bank, zero/oversized limit, or storage failures.
    fn inspect_items(&self, filter: &ItemInspectFilter) -> Result<Vec<ItemInspectRow>, AmError>;

    /// Total number of items matching a filter's predicates, ignoring `limit`/`offset`.
    ///
    /// The inspector pairs this with [`Store::inspect_items`] so a page knows
    /// the full result size without a second page request.
    ///
    /// # Errors
    /// Empty bank, invalid filter, or storage failures.
    fn count_items(&self, filter: &ItemInspectFilter) -> Result<usize, AmError>;

    /// Atomic discrete correction: close the prior assertion on both time axes, insert the
    /// corrected assertion linked via `corrects_item_id`/`superseded_by_id`, and write the
    /// `correct` telemetry rows inside the same transaction. Never `DELETE`s.
    ///
    /// # Errors
    /// Missing or cross-bank prior, already-discarded prior, validation, or
    /// storage failures.
    fn correct_item(&self, spec: &CorrectionSpec) -> Result<CorrectionOutcome, AmError>;

    /// Reconstruct the full lifecycle of one item id from write-path
    /// structures: telemetry rows, revision rows, lineage pointers, and the
    /// belief confidence trajectory when the target is a belief.
    ///
    /// # Errors
    /// `ItemNotFound` when the id is absent from the caller's bank, or storage failures.
    fn audit_trail(&self, bank_id: &str, item_id: &str) -> Result<ItemAuditTrail, AmError>;

    /// Every triple row in the bank, oldest transaction time first, including
    /// closed/superseded edges (bi-temporal history is part of the payload).
    ///
    /// # Errors
    /// Storage failures.
    fn list_triples(&self, bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;

    /// Atomically create a distilled item and its item-keyed derivation record in one
    /// transaction; a failure leaves neither behind. Never resolved through `(bank, subject)`.
    /// # Errors: validation, mismatch, duplicate key, crypto, or storage.
    fn commit_distilled_unit(
        &self,
        item: &MemoryItem,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError>;

    /// Read one distilled unit's derivation record, bank-scoped. `Ok(None)` = no
    /// record for that item id in that bank; storage failures return an error.
    fn get_distilled_unit(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<DistilledUnitRecord>, AmError>;
}

#[cfg(test)]
#[path = "store_admission_gated_tests.rs"]
mod store_admission_gated_tests;

#[cfg(test)]
#[path = "store_parity_tests.rs"]
mod store_parity_tests;
