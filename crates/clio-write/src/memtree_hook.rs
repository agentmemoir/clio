// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for persisting MemTree rows: the leaf-attach hook
//! and the upsert of every node a write or a refresh wave changed.
//!
//! ## Owns
//! - `MaintenanceHook` for `MemTreeMaint`: attach the leaf, mark ancestors,
//!   then upsert the rows the attach created or changed
//! - The store handle carried by the engine, and the constructor that wires it
//! - The write-time stamp a persisted row carries
//!
//! ## Does not own
//! - Attach / split topology or dirty-path refresh (siblings in `memtree_maint`)
//! - The node/row shape conversion (`memtree_record`)
//! - Whether a caller waits for a dispatched signal (`maintenance`)
//!
//! ## Boundary
//!
//! Changes to which nodes get persisted, or when, should normally require
//! changes here.
//! Changes to refresh scheduling or the SQL behind a row should not require
//! changes here.
//!
//! Keep this module focused on making the in-memory tree durable.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::AttachLeaf;
use crate::memtree_maint::{MemTreeMaint, SharedMemForest};
use crate::memtree_record::record_from_node;
use clio_store::{MemTreeNodeRecord, Store};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

impl std::fmt::Debug for MemTreeMaint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemTreeMaint")
            .field("durable", &self.store.is_some())
            .field("waves_in_flight", &self.waves_in_flight())
            .finish_non_exhaustive()
    }
}

impl MemTreeMaint {
    /// Build a maintainer that also persists what it attaches and refreshes.
    #[must_use]
    pub fn with_store(
        forest: SharedMemForest,
        leaves: Arc<LeafIndex>,
        store: Arc<dyn Store>,
    ) -> Arc<Self> {
        Self::with_store_handle(forest, leaves, Some(store))
    }

    /// Upsert the rows of `node_ids` as they stand in the forest now.
    ///
    /// Reads under the forest lock and writes outside it: the stored row is the
    /// node's state at the moment it was read, and no store call runs while the
    /// structural lock is held. A store-less engine does nothing.
    pub(crate) fn persist_nodes(&self, bank_id: &str, node_ids: &[String]) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        if node_ids.is_empty() {
            return;
        }
        let now = now_stamp();
        let records: Vec<MemTreeNodeRecord> = {
            let forest = self.forest.lock().expect("forest");
            let Some(tree) = forest.tree(bank_id) else {
                return;
            };
            node_ids
                .iter()
                .filter_map(|id| tree.get(id))
                .map(|node| record_from_node(node, &now))
                .collect()
        };
        upsert_all(store, records);
    }

    /// Upsert the leaf an attach placed plus every node the attach changed.
    ///
    /// The attach reports the ancestors it dirtied, which is not the whole
    /// change set: a fan-out split also re-parents the children it moved and can
    /// insert a sibling and a new root. So the leaf, the dirtied ids, the leaf's
    /// ancestor chain and each of those nodes' children are persisted. Anything
    /// outside that set kept its row's columns, and a stale row for a node no
    /// longer reachable is unreachable for a reader too.
    fn persist_attach(&self, bank_id: &str, leaf_id: &str, dirty: &[String]) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let now = now_stamp();
        let records: Vec<MemTreeNodeRecord> = {
            let forest = self.forest.lock().expect("forest");
            let Some(tree) = forest.tree(bank_id) else {
                return;
            };
            let mut ids = Vec::with_capacity(dirty.len().saturating_mul(2).saturating_add(2));
            ids.push(leaf_id.to_owned());
            ids.extend(dirty.iter().cloned());
            ids.extend(tree.ancestors_of(leaf_id));
            let moved: Vec<String> = ids
                .iter()
                .filter_map(|id| tree.get(id))
                .flat_map(|node| node.child_ids.iter().cloned())
                .collect();
            ids.extend(moved);
            ids.sort();
            ids.dedup();
            ids.iter()
                .filter_map(|id| tree.get(id))
                .map(|node| record_from_node(node, &now))
                .collect()
        };
        upsert_all(store, records);
    }
}

impl MaintenanceHook for MemTreeMaint {
    fn on_leaf_attached(&self, signal: MaintenanceSignal) {
        let Some(item) = self.leaves.get(&signal.bank_id, &signal.item_id) else {
            return;
        };
        let attach = AttachLeaf {
            item_id: signal.item_id.clone(),
            bank_id: signal.bank_id.clone(),
            interval_start: item.created_at.clone(),
            interval_end: item.created_at,
            gist: item.gist,
            task_id: None,
        };
        let (leaf_id, dirty) = {
            let mut forest = self.forest.lock().expect("forest");
            let tree = forest.tree_mut(&signal.bank_id);
            if let Ok(attached) = tree.attach_leaf(attach) {
                attached
            } else {
                // A write path that places the leaf itself and dispatches the
                // signal afterwards gets here with the leaf already placed. That
                // is not a reason to persist nothing: the node the first attach
                // created is exactly the one this signal reports, so the rows are
                // taken from the tree as it stands.
                let Some(leaf_id) = tree.leaf_node_id_for_item(&signal.item_id) else {
                    return;
                };
                let dirty = tree.ancestors_of(leaf_id);
                (leaf_id.to_owned(), dirty)
            }
        };
        self.enqueue_dirty(&signal.bank_id, &dirty);
        self.persist_attach(&signal.bank_id, &leaf_id, &dirty);
    }
}

/// The store time stamp for a row written now, in the store's canonical form.
fn now_stamp() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    clio_store::millis_to_rfc3339(millis)
}

/// Write the rows one by one.
///
/// A failed upsert is not reported upwards: the hook runs after the write has
/// been answered and has no caller to fail, and the node keeps its dirty mark,
/// so the next wave re-reads it from the forest and tries again.
fn upsert_all(store: &dyn Store, records: Vec<MemTreeNodeRecord>) {
    for record in records {
        let _ = store.upsert_memtree_node(&record);
    }
}

#[cfg(test)]
#[path = "memtree_hook_tests.rs"]
mod memtree_hook_tests;
