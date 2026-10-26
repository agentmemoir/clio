// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree node types and per-bank forest handles.
//!
//! ## Owns
//! - `MemTreeNode`, attach/query DTOs, default fan-out constant
//! - `BankMemTree` / `MemForest` containers and read accessors
//!
//! ## Does not own
//! - Structural insert / split algorithms
//! - Dirty-path refresh scheduling
//!
//! ## Boundary
//!
//! Changes to MemTree DTOs and container APIs should normally require changes here.
//! Changes to attach/split algorithms should not require changes here.
//!
//! Keep this module focused on MemTree types and read-side bank tree access.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default max children per internal node (MemForest-aligned).
pub const DEFAULT_FANOUT: usize = 8;

/// One MemTree node (leaf or internal aggregate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemTreeNode {
    /// Stable node id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Parent node (`None` = root).
    pub parent_id: Option<String>,
    /// Leaf item id when this is a leaf.
    pub item_id: Option<String>,
    /// Depth: 0 = leaf; increases toward root.
    pub depth: u32,
    /// Interval start (sortable text).
    pub interval_start: String,
    /// Interval end (sortable text).
    pub interval_end: String,
    /// Ordered child node ids (empty for leaves).
    pub child_ids: Vec<String>,
    /// Aggregate gist (non-authoritative).
    pub summary_gist: Option<String>,
    /// Dirty flag for derived summary.
    pub dirty: bool,
    /// Generation bumped on each dirty mark.
    pub generation: u64,
    /// Refresh in flight.
    pub refreshing: bool,
    /// Optional task association.
    pub task_id: Option<String>,
    /// Last refresh error.
    pub last_error: Option<String>,
}

impl MemTreeNode {
    /// True when this node holds a leaf item reference.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.item_id.is_some() && self.depth == 0
    }
}

/// Inputs for attaching an admitted ingest leaf into the tree.
#[derive(Debug, Clone)]
pub struct AttachLeaf {
    /// Admitted item id.
    pub item_id: String,
    /// Bank id (must match tree bank).
    pub bank_id: String,
    /// Sortable interval start (usually `created_at`).
    pub interval_start: String,
    /// Sortable interval end.
    pub interval_end: String,
    /// Leaf gist seed (optional).
    pub gist: Option<String>,
    /// Optional task id.
    pub task_id: Option<String>,
}

/// Filters for hierarchical MemTree reads.
#[derive(Debug, Clone, Default)]
pub struct MemTreeQueryFilter {
    /// Overlap start (inclusive).
    pub time_start: Option<String>,
    /// Overlap end (inclusive).
    pub time_end: Option<String>,
    /// Exact depth filter.
    pub depth: Option<u32>,
    /// Task id filter.
    pub task_id: Option<String>,
    /// Max results (default 100).
    pub limit: Option<usize>,
}

/// Per-bank hierarchical temporal index.
#[derive(Debug)]
pub struct BankMemTree {
    pub(crate) bank_id: String,
    pub(crate) fanout: usize,
    pub(crate) root_id: Option<String>,
    pub(crate) nodes: HashMap<String, MemTreeNode>,
    /// item_id → leaf node id
    pub(crate) placement: HashMap<String, String>,
    /// Node ids whose parent this most recent attach changed by splitting: the
    /// children a split moved and the two halves a new root took. Awaiting a
    /// durable write of their new `parent_id`; emptied at the start of every
    /// attach, so it holds at most one attach's moves.
    pub(crate) reparented: Vec<String>,
    pub(crate) seq: AtomicU64,
}

impl BankMemTree {
    /// Empty tree for a bank.
    #[must_use]
    pub fn new(bank_id: impl Into<String>, fanout: usize) -> Self {
        Self {
            bank_id: bank_id.into(),
            fanout: fanout.max(2),
            root_id: None,
            nodes: HashMap::new(),
            placement: HashMap::new(),
            reparented: Vec::new(),
            seq: AtomicU64::new(1),
        }
    }

    /// Bank isolation key for this tree.
    #[must_use]
    pub fn bank_id(&self) -> &str {
        &self.bank_id
    }

    /// Current root node id, if any leaves have been attached.
    #[must_use]
    pub fn root_id(&self) -> Option<&str> {
        self.root_id.as_deref()
    }

    /// Max children per internal node.
    #[must_use]
    pub fn fanout(&self) -> usize {
        self.fanout
    }

    pub(crate) fn next_id(&self, kind: &str) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        format!("{}:{kind}:{n}", self.bank_id)
    }

    /// Look up a node by id (bank-local).
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<&MemTreeNode> {
        self.nodes.get(node_id)
    }

    /// Placement map lookup.
    #[must_use]
    pub fn leaf_node_id_for_item(&self, item_id: &str) -> Option<&str> {
        self.placement.get(item_id).map(String::as_str)
    }

    /// Ancestor chain from node toward root (excluding the start node).
    #[must_use]
    pub fn ancestors_of(&self, node_id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.nodes.get(node_id).and_then(|n| n.parent_id.clone());
        while let Some(id) = cur {
            out.push(id.clone());
            cur = self.nodes.get(&id).and_then(|n| n.parent_id.clone());
        }
        out
    }

    /// Mutable access for maintenance (dirty / refresh).
    pub fn node_mut(&mut self, node_id: &str) -> Option<&mut MemTreeNode> {
        self.nodes.get_mut(node_id)
    }

    /// All node ids currently marked dirty.
    #[must_use]
    pub fn dirty_node_ids(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| n.dirty)
            .map(|n| n.id.clone())
            .collect()
    }

    /// Query nodes with filters.
    #[must_use]
    pub fn query(&self, filter: &MemTreeQueryFilter) -> Vec<MemTreeNode> {
        let limit = filter.limit.unwrap_or(100).min(1000);
        let mut out: Vec<_> = self
            .nodes
            .values()
            .filter(|n| {
                if let Some(ref start) = filter.time_start
                    && n.interval_end.as_str() < start.as_str()
                {
                    return false;
                }
                if let Some(ref end) = filter.time_end
                    && n.interval_start.as_str() > end.as_str()
                {
                    return false;
                }
                if let Some(d) = filter.depth
                    && n.depth != d
                {
                    return false;
                }
                if let Some(ref task) = filter.task_id
                    && n.task_id.as_deref() != Some(task.as_str())
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.interval_start
                .cmp(&b.interval_start)
                .then_with(|| a.id.cmp(&b.id))
        });
        out.truncate(limit);
        out
    }

    /// Force-dirty all internal nodes (repair aid).
    pub fn force_dirty_all_internal(&mut self) -> Vec<String> {
        let mut ids = Vec::new();
        for n in self.nodes.values_mut() {
            if n.depth > 0 {
                n.dirty = true;
                n.generation = n.generation.saturating_add(1);
                ids.push(n.id.clone());
            }
        }
        ids
    }
}

/// Multi-bank MemTree forest (one primary episodic tree per bank).
#[derive(Debug, Default)]
pub struct MemForest {
    trees: HashMap<String, BankMemTree>,
    fanout: usize,
}

impl MemForest {
    /// Forest with default fan-out.
    #[must_use]
    pub fn new() -> Self {
        Self::with_fanout(DEFAULT_FANOUT)
    }

    /// Forest with explicit fan-out.
    #[must_use]
    pub fn with_fanout(fanout: usize) -> Self {
        Self {
            trees: HashMap::new(),
            fanout: fanout.max(2),
        }
    }

    /// Get or create the bank tree.
    pub fn tree_mut(&mut self, bank_id: &str) -> &mut BankMemTree {
        let fanout = self.fanout;
        self.trees
            .entry(bank_id.to_owned())
            .or_insert_with(|| BankMemTree::new(bank_id, fanout))
    }

    /// Immutable bank tree.
    #[must_use]
    pub fn tree(&self, bank_id: &str) -> Option<&BankMemTree> {
        self.trees.get(bank_id)
    }
}

#[cfg(test)]
#[path = "memtree_tests.rs"]
mod memtree_tests;

#[cfg(test)]
#[path = "memtree_cov_tests.rs"]
mod memtree_cov_tests;
