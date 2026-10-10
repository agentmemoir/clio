// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for refreshing one MemTree node inside a wave,
//! without holding the forest lock while the summary is computed.
//!
//! ## Owns
//! - The three-phase lock/aggregate/lock sequence for a single node
//! - The generation guard that keeps a mid-wave re-dirty out of this wave
//! - `RefreshOutcome`, the per-node verdict the wave aggregates
//!
//! ## Does not own
//! - Wave scheduling, the coalesced dirty set, or the worker pool
//!   (`memtree_maint.rs`)
//! - Tree topology and summary formatting (`memtree.rs`)
//!
//! ## Boundary
//!
//! Split out of `memtree_maint.rs` to keep that file inside the 450-line limit.
//! Changes to the per-node generation guard belong here; changes to how many
//! nodes run at once belong in `memtree_maint.rs`.
//!
//! Keep this module focused on one node's refresh.

use super::{MemTreeMaint, RefreshOutcome, SharedMemForest};
use crate::memtree::MemTreeNode;
use std::collections::HashSet;

/// Refresh one node in three phases so the summary is computed outside the
/// forest lock: (1) brief lock to validate the wave generation and snapshot
/// children, (2) summary aggregation without the lock, (3) brief lock to
/// re-validate and commit.
pub(crate) fn refresh_one(
    forest: &SharedMemForest,
    bank_id: &str,
    node_id: &str,
    wave_generation: u64,
    fail: &HashSet<String>,
) -> RefreshOutcome {
    if fail.contains(node_id) {
        return RefreshOutcome::Failed(format!("injected refresh failure for `{node_id}`"));
    }
    let children = {
        let mut guard = forest.lock().expect("forest");
        let tree = guard.tree_mut(bank_id);
        let Some(n) = tree.node_mut(node_id) else {
            return RefreshOutcome::Failed(format!("missing node `{node_id}`"));
        };
        if n.generation != wave_generation {
            // Re-dirtied after the wave snapshot; leave it for the next wave.
            return RefreshOutcome::Superseded;
        }
        n.refreshing = true;
        let child_ids = n.child_ids.clone();
        child_ids
            .iter()
            .filter_map(|cid| tree.get(cid).cloned())
            .collect::<Vec<MemTreeNode>>()
    };
    // Summary aggregation runs without the forest lock.
    let refs: Vec<&MemTreeNode> = children.iter().collect();
    let summary = MemTreeMaint::summarize_children(&refs);

    let mut guard = forest.lock().expect("forest");
    let tree = guard.tree_mut(bank_id);
    let (generation, was_dirty) = match tree.get(node_id) {
        Some(n) => (n.generation, n.dirty),
        None => return RefreshOutcome::Superseded,
    };
    if generation != wave_generation {
        // A new write re-dirtied this node mid-wave; keep it queued.
        tree.node_mut(node_id).expect("present").refreshing = false;
        return RefreshOutcome::Superseded;
    }
    if !was_dirty {
        // A concurrent wave already refreshed this node; nothing left to do.
        return RefreshOutcome::Refreshed;
    }
    // Interval from children; never touch child snapshots (leaves hold item refs only).
    tree.refresh_interval_from_children(node_id);
    if let Some(n) = tree.node_mut(node_id) {
        n.summary_gist = Some(summary);
        n.dirty = false;
        n.refreshing = false;
        n.last_error = None;
    }
    RefreshOutcome::Refreshed
}
