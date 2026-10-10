// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-mostly operator health contract
//! shared by both persistence backends: reachability, schema version, bank
//! inventory, and content-free referential-integrity signals.
//!
//! ## Owns
//! - `OpsStore` behavioral contract (ping, schema version, bank ids, item and
//!   subject counts, integrity signal counts, orphan edge cleanup)
//! - `IntegritySignals` count shape (ids/counts only; never memory content)
//! - `StoreCounts`: the memory and subject totals the setup switch gate reads
//!
//! ## Does not own
//! - Backend SQL statements (see `sqlite_ops` / `postgres_ops`)
//! - Finding severity, plan composition, or repair allowlists (the ops layer)
//!
//! ## Boundary
//!
//! Changes to the operator health contract surface should normally require
//! changes here. Changes to driver SQL or ops policy should not require changes
//! here.
//!
//! Keep this module focused on the backend-agnostic operator health contract.

use clio_types::AmError;
use serde::{Deserialize, Serialize};

use crate::embedding_space::EmbeddingSpace;

/// Content-free referential-integrity signal counts for one scope.
///
/// Every field is a row count; no column value is ever returned, so the shape
/// is safe to emit in operator output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IntegritySignals {
    /// `assoc_edges` rows whose `source_id` or `target_id` is not a live item id.
    pub orphan_assoc_edges: u64,
    /// `triples` rows whose `item_id` is not an item id.
    pub orphan_triple_items: u64,
    /// `triples` rows with a non-null `snapshot_ref` that is not an item id.
    pub dangling_snapshot_refs: u64,
    /// `triples` rows with a non-null `gist_ref` that is not an item id.
    pub dangling_gist_refs: u64,
    /// `memtree_nodes` leaf rows whose non-null `item_id` is not an item id.
    pub orphan_memtree_leaf_items: u64,
}

/// How many memories and subjects one store holds.
///
/// Two counts, no content: the setup wizard's switch gate names them in plain
/// words before it lets a database change through, and a count is all it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StoreCounts {
    /// Rows in the item table.
    pub memories: u64,
    /// Distinct non-null `subject_id` values in the item table.
    pub subjects: u64,
}

/// Operator health repository contract (both backends share one shape).
///
/// All reads are bank-scoped when a bank is given and store-wide otherwise.
/// Counts only: implementations MUST NOT return item content, refs, or hashes.
pub trait OpsStore: Send + Sync {
    /// Count the memories and the distinct subjects in this store.
    ///
    /// Store-wide, never bank-scoped: "does changing the database lose
    /// anything" is a question about the whole store.
    ///
    /// # Errors
    /// Storage failures.
    fn counts(&self) -> Result<StoreCounts, AmError>;

    /// Backend reachability probe (a minimal read that fails when the store is
    /// unavailable).
    ///
    /// # Errors
    /// Connection or query failures.
    fn ping(&self) -> Result<(), AmError>;

    /// Recorded schema version, or `None` when the meta row is absent.
    ///
    /// # Errors
    /// Storage failures.
    fn schema_version(&self) -> Result<Option<String>, AmError>;

    /// The active embedding space `(model_id, dims)` this store serves.
    ///
    /// # Errors
    /// Storage failures.
    fn embedding_space(&self) -> Result<EmbeddingSpace, AmError>;

    /// Distinct bank ids seen in the item table, sorted.
    ///
    /// # Errors
    /// Storage failures.
    fn bank_ids(&self) -> Result<Vec<String>, AmError>;

    /// Content-free integrity signal counts for the requested scope.
    ///
    /// # Errors
    /// Storage failures.
    fn integrity_signals(&self, bank: Option<&str>) -> Result<IntegritySignals, AmError>;

    /// Delete association edges whose endpoint item no longer exists, restricted
    /// to `bank`. Returns the number of edge rows removed.
    ///
    /// # Errors
    /// Empty bank or storage failures.
    fn drop_orphan_assoc_edges(&self, bank: &str) -> Result<u64, AmError>;
}

#[cfg(test)]
#[path = "ops_store_tests.rs"]
mod ops_store_tests;
