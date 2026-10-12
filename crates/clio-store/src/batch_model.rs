// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the atomic batch write contract: the
//! pre-validated write payloads a batch may stage and the `BatchStore` trait
//! that commits them all-or-nothing in one backend transaction.
//!
//! ## Owns
//! - `BatchWrite` payload enum (post-gating row-level effects)
//! - `BatchStore` transactional executor contract
//!
//! ## Does not own
//! - Admission gating, validation ordering, or `$ref` resolution (callers stage
//!   fully admitted payloads; this module never re-scores)
//! - Per-backend SQL (see `batch_sqlite` / `batch_postgres`)
//!
//! ## Boundary
//!
//! Changes to which effects a batch may commit should normally require changes
//! here and in the per-backend `*_batch` modules. Changes to MCP op decoding
//! should not require changes here.
//!
//! Keep this module focused on the batch write contract.

use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, FailureRecord, MemoryItem, PersonaPreference,
    PersonaStableEntry, TaskRecord, TripleRecord,
};
use serde_json::Value;

/// Cap on operations per batch transaction (lock-contention guard).
pub const MAX_BATCH_OPS: usize = 50;

/// One pre-validated, post-gating write effect inside a batch.
///
/// Every payload arrives fully admitted (category gate + five-factor scoring
/// already passed upstream); the executor never re-scores. Row-level effects
/// mirror the single-op store internals so a batch commits in one transaction.
#[derive(Debug, Clone)]
pub enum BatchWrite {
    /// Admitted item create (`store` new revision).
    CreateItem {
        /// Admitted item (score stamped).
        item: MemoryItem,
        /// Attribution actor.
        actor: String,
    },
    /// Admitted canonical revision (single-slot `canonical_put`): the executor
    /// resolves the active slot row for the item's subject inside the
    /// transaction, closes it on both time axes, inserts the new revision, and
    /// appends the `canonical_put` audit marker. Resolving the prior at commit
    /// time (not pre-flight) keeps two puts on one key in one batch down to a
    /// single active revision.
    CanonicalPut {
        /// Admitted item (score stamped; `subject_id` is `canonical:{key}`).
        item: MemoryItem,
        /// Canonical key (audit detail only).
        key: String,
        /// Attribution actor.
        actor: String,
    },
    /// Admitted triple carrier + SPO insert with optional supersession.
    CommitTriple {
        /// Admitted carrier item.
        item: MemoryItem,
        /// SPO edge referencing the carrier.
        edge: TripleRecord,
        /// Close prior open subject+predicate edges.
        supersede: bool,
        /// Attribution actor.
        actor: String,
    },
    /// Close both time axes on an item row (canonical supersession prior).
    CloseItem {
        /// Bank isolation key.
        bank_id: String,
        /// Item id to close.
        item_id: String,
        /// Valid-time close stamp.
        valid_until: String,
        /// Transaction-time close stamp.
        tx_until: String,
        /// Optional superseding item id.
        replacement_id: Option<String>,
        /// Attribution actor.
        actor: String,
    },
    /// Gated task history upsert (carrier + structured row).
    CommitTaskUpsert {
        /// Admitted carrier item.
        item: MemoryItem,
        /// Structured task record draft.
        record: TaskRecord,
        /// Attribution actor.
        actor: String,
    },
    /// Gated failure history insert (carrier + structured record).
    CommitFailureRecord {
        /// Admitted carrier item.
        item: MemoryItem,
        /// Structured failure record draft.
        record: FailureRecord,
        /// Attribution actor.
        actor: String,
    },
    /// Gated persona stable-entry upsert.
    PutStableEntry {
        /// Admitted persona entry.
        entry: PersonaStableEntry,
        /// Attribution actor.
        actor: String,
    },
    /// Force-overwrite of an existing typed item from a bundle import.
    ///
    /// Same mutable-field update as `update_memory_item`, moved inside the
    /// batch transaction so an import plans without mutating the store.
    UpsertMemoryItem {
        /// Incoming item (masked before staging).
        item: MemoryItem,
        /// Attribution actor.
        actor: String,
    },
    /// Restore one `ciphertext_backup` opaque row verbatim.
    ///
    /// Writes the exporting node's ciphertext to `content_ciphertext` without
    /// re-encrypting and preserves every metadata column.
    PutOpaqueItem {
        /// Opaque item row to upsert.
        item: crate::model::OpaqueItemRecord,
        /// Attribution actor.
        actor: String,
    },
    /// Persona continuous-preference upsert (`put_preference_state`).
    PutPreferenceState {
        /// Preference state to upsert.
        preference: PersonaPreference,
        /// Attribution actor.
        actor: String,
    },
    /// Gated belief create with its initial trajectory.
    CreateBelief {
        /// Belief identity plus initial confidence history.
        belief: BeliefObject,
        /// Attribution actor.
        actor: String,
    },
    /// Append one confidence entry to an existing belief.
    AppendBeliefConfidence {
        /// Bank isolation key.
        bank_id: String,
        /// Existing belief id.
        belief_id: String,
        /// Confidence entry to append.
        entry: ConfidenceEntry,
        /// Attribution actor.
        actor: String,
    },
    /// Explicit association-edge upsert (graph metadata, not admission-gated).
    PutAssocEdge {
        /// Edge to insert or replace.
        edge: crate::model::AssocEdgeRecord,
        /// Attribution actor.
        actor: String,
    },
    /// PII-safe audit-event append inside the same transaction (e.g. a
    /// reviewer attestation or shared-bank telemetry row).
    AuditEvent {
        /// Bank isolation key.
        bank_id: String,
        /// Operation label.
        operation: String,
        /// Optional item id reference.
        item_id: Option<String>,
        /// Attribution actor.
        actor: String,
        /// Ids/counts detail (never raw payloads).
        detail: Value,
    },
}

impl BatchWrite {
    /// Target bank of the effect (tenant-boundary sanity for the executor).
    #[must_use]
    pub fn bank_id(&self) -> &str {
        match self {
            Self::CreateItem { item, .. } | Self::CommitTriple { item, .. } => &item.bank_id,
            Self::CanonicalPut { item, .. } => &item.bank_id,
            Self::CloseItem { bank_id, .. } | Self::AuditEvent { bank_id, .. } => bank_id,
            Self::CommitTaskUpsert { item, .. } | Self::CommitFailureRecord { item, .. } => {
                &item.bank_id
            }
            Self::PutStableEntry { entry, .. } => &entry.bank_id,
            Self::PutAssocEdge { edge, .. } => &edge.bank_id,
            Self::UpsertMemoryItem { item, .. } => &item.bank_id,
            Self::PutOpaqueItem { item, .. } => &item.bank_id,
            Self::PutPreferenceState { preference, .. } => &preference.bank_id,
            Self::CreateBelief { belief, .. } => &belief.bank_id,
            Self::AppendBeliefConfidence { bank_id, .. } => bank_id,
        }
    }

    /// The item this write creates, when it creates one.
    ///
    /// `None` for a write that only closes a row, attaches an edge, or records
    /// metadata, and for a `ciphertext_backup` opaque row: an opaque row carries
    /// no dual content, so there is nothing to index.
    #[must_use]
    pub fn created_item(&self) -> Option<&MemoryItem> {
        match self {
            Self::CreateItem { item, .. }
            | Self::CanonicalPut { item, .. }
            | Self::CommitTriple { item, .. }
            | Self::CommitTaskUpsert { item, .. }
            | Self::CommitFailureRecord { item, .. }
            | Self::UpsertMemoryItem { item, .. } => Some(item),
            _ => None,
        }
    }
}

/// Transactional batch commit contract (both backends).
///
/// Implementations MUST execute every write inside ONE database transaction:
/// on any failure nothing persists (drop-rollback), on success all rows are
/// visible together. Admission already happened upstream — this is commit-only.
pub trait BatchStore: Send + Sync {
    /// Apply all writes atomically; `Ok(())` means every effect committed.
    ///
    /// # Errors
    /// Any validation, crypto, or storage failure rolls back the whole batch.
    fn apply_batch(&self, writes: &[BatchWrite]) -> Result<(), AmError>;
}

#[cfg(test)]
#[path = "batch_model_tests.rs"]
mod batch_model_tests;
