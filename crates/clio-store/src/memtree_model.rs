// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree persistence record shapes.
//!
//! ## Owns
//! - `MemTreeNodeRecord` and `MemTreeQuery` filter structs
//!
//! ## Does not own
//! - Attach / dirty-path / refresh algorithms
//!
//! ## Boundary
//!
//! Changes to durable MemTree row shape should normally require changes here.
//! Changes to in-process tree maintenance should not require changes here.
//!
//! Keep this module focused on MemTree persistence DTOs.

use serde::{Deserialize, Serialize};

/// Durable MemTree node row (bank-scoped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemTreeNodeRecord {
    /// Node id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Parent node id (`None` = root).
    pub parent_id: Option<String>,
    /// Leaf item id when this node is a leaf.
    pub item_id: Option<String>,
    /// Depth (0 = leaf).
    pub depth: i32,
    /// Inclusive interval start (sortable text).
    pub interval_start: String,
    /// Inclusive interval end (sortable text).
    pub interval_end: String,
    /// Ordered child node ids.
    pub child_ids: Vec<String>,
    /// Aggregate gist (non-authoritative).
    pub summary_gist: Option<String>,
    /// Dirty flag.
    pub dirty: bool,
    /// Dirty generation counter.
    pub generation: i64,
    /// Refresh-in-flight flag.
    pub refreshing: bool,
    /// Optional task association.
    pub task_id: Option<String>,
    /// Last refresh error (if any).
    pub last_error: Option<String>,
    /// Created timestamp.
    pub created_at: String,
    /// Updated timestamp.
    pub updated_at: String,
}

/// Filter for MemTree listing.
#[derive(Debug, Clone, Default)]
pub struct MemTreeQuery {
    /// Bank scope (required by callers).
    pub bank_id: String,
    /// Optional interval overlap start.
    pub time_start: Option<String>,
    /// Optional interval overlap end.
    pub time_end: Option<String>,
    /// Optional exact depth.
    pub depth: Option<i32>,
    /// Optional task id.
    pub task_id: Option<String>,
    /// Max rows (default applied by store).
    pub limit: Option<usize>,
}

#[cfg(test)]
#[path = "memtree_model_persist_tests.rs"]
mod memtree_model_persist_tests;
