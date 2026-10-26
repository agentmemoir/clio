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
//! - The persist-failure record (count and last message) a row write that did
//!   not land leaves on the engine
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
use clio_types::AmError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Persistence failures the write path hit, kept so a later status read reports
/// them.
#[derive(Debug, Default)]
pub(crate) struct PersistErrors {
    /// How many row writes (or dispatch waits) did not land.
    count: AtomicU64,
    /// The message of the most recent failure.
    last_error: Mutex<Option<String>>,
}

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
    /// structural lock is held. A store-less engine does nothing apart from the
    /// empty-list short circuit. Returns the first store error, which the caller
    /// records on the engine rather than discarding.
    pub(crate) fn persist_nodes(&self, bank_id: &str, node_ids: &[String]) -> Result<(), AmError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        if node_ids.is_empty() {
            return Ok(());
        }
        let now = now_stamp();
        let records: Vec<MemTreeNodeRecord> = {
            let forest = self.forest.lock().expect("forest");
            let Some(tree) = forest.tree(bank_id) else {
                return Ok(());
            };
            node_ids
                .iter()
                .filter_map(|id| tree.get(id))
                .map(|node| record_from_node(node, &now))
                .collect()
        };
        upsert_all(store, records)
    }

    /// Upsert the leaf an attach placed plus every node the attach changed.
    ///
    /// The attach reports the ancestors it dirtied, which is not the whole
    /// change set: a fan-out split also re-parents the children it moved and can
    /// insert a sibling and a new root. So the leaf, the dirtied ids, the leaf's
    /// ancestor chain and each of those nodes' children are persisted. Anything
    /// outside that set kept its row's columns, and a stale row for a node no
    /// longer reachable is unreachable for a reader too.
    fn persist_attach(
        &self,
        bank_id: &str,
        leaf_id: &str,
        dirty: &[String],
    ) -> Result<(), AmError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let now = now_stamp();
        let records: Vec<MemTreeNodeRecord> = {
            let forest = self.forest.lock().expect("forest");
            let Some(tree) = forest.tree(bank_id) else {
                return Ok(());
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
        upsert_all(store, records)
    }

    /// Record a row write, or a dispatch wait, that did not land.
    ///
    /// The item write has already been answered when this runs, so the
    /// degradation is disclosed instead of rolling the write back: the process
    /// log names it now, and the engine keeps a count and the message for a
    /// later `maintenance status`. Without this the process exited 0 over rows
    /// that were never written and nothing anywhere said so.
    pub fn record_persist_failure(&self, error: &AmError) {
        clio_config::log::warn(
            "memtree_persist_failed",
            &[
                ("code", error.code.as_str()),
                ("error", error.message.as_str()),
            ],
        );
        self.persist_errors.count.fetch_add(1, Ordering::Relaxed);
        *self
            .persist_errors
            .last_error
            .lock()
            .expect("persist error") = Some(error.message.clone());
    }

    /// How many persist failures this engine recorded, and the last message.
    #[must_use]
    pub fn persist_failures(&self) -> (u64, Option<String>) {
        (
            self.persist_errors.count.load(Ordering::Relaxed),
            self.persist_errors
                .last_error
                .lock()
                .expect("persist error")
                .clone(),
        )
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
        if let Err(error) = self.persist_attach(&signal.bank_id, &leaf_id, &dirty) {
            self.record_persist_failure(&error);
        }
    }
}

/// The store time stamp for a row written now, in the store's canonical form.
fn now_stamp() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    clio_store::millis_to_rfc3339(millis)
}

/// Write the rows one by one, stopping at the first failure.
///
/// The error is returned, not dropped: the hook runs after the write has been
/// answered and has no caller to fail, so the only honest thing left is to hand
/// the error to [`MemTreeMaint::record_persist_failure`] at the call site. The
/// node keeps its dirty mark, but that alone retries only what a refresh wave
/// processes, which is not the leaf this upsert belongs to.
fn upsert_all(store: &dyn Store, records: Vec<MemTreeNodeRecord>) -> Result<(), AmError> {
    for record in records {
        store.upsert_memtree_node(&record)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "memtree_hook_tests.rs"]
mod memtree_hook_tests;
