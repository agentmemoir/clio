// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for leaf publish state and non-blocking maintenance handoff.
//!
//! ## Owns
//! - In-process leaf index (get-by-id / list) independent of ancestor refresh
//! - Leaf-publish timing retained for the status surface
//! - `LeafAttached` maintenance signal and the bounded wait a write may take on
//!   the detached hook that consumes it
//!
//! ## Does not own
//! - MemTree ancestor recompute or hybrid retrieve
//!
//! ## Boundary
//!
//! Changes to leaf-first readability / maintenance hook shape should normally require changes here.
//! Changes to MemTree ancestor recompute or hybrid retrieve should not require changes here.
//!
//! Keep this module focused on leaf readability and non-blocking maintenance handoff.

use clio_types::MemoryItem;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Signal emitted after a leaf is admitted and marked queryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceSignal {
    /// Always `leaf_attached` for this slice.
    pub kind: &'static str,
    /// Admitted item id.
    pub item_id: String,
    /// Bank isolation key.
    pub bank_id: String,
}

impl MaintenanceSignal {
    /// Build a leaf-attached signal.
    #[must_use]
    pub fn leaf_attached(item_id: impl Into<String>, bank_id: impl Into<String>) -> Self {
        Self {
            kind: "leaf_attached",
            item_id: item_id.into(),
            bank_id: bank_id.into(),
        }
    }
}

/// Consumer for dirty-path refresh, which a later stage may subscribe.
///
/// Contract: a hook runs on the dispatcher thread, and a write may wait for it
/// only through [`DispatchJob::drain`], bounded by a budget the caller chooses.
/// A hook therefore owns the leaf's own durability but never runs hierarchical
/// consolidation: per-signal work must stay small enough for that budget.
/// Implementors MUST be `Send + Sync` so they can be shared with the dispatcher.
pub trait MaintenanceHook: std::fmt::Debug {
    /// Notify that a leaf is durable and queryable; path may be dirty.
    fn on_leaf_attached(&self, signal: MaintenanceSignal);
}

/// Thread-safe handle to a maintenance hook for non-blocking dispatch.
pub type SharedMaintenanceHook = Arc<dyn MaintenanceHook + Send + Sync>;

/// Delivery budget a write uses when it waits for its own signals.
///
/// The hook's own work is one leaf attach plus a row set bounded by the leaf's
/// depth, so 1500 ms is generous for it on a loaded host while staying inside
/// the window a leaf write has to become queryable in: a wait that could outlast
/// that window would let a slow store break the readability promise instead of
/// only delaying one write.
pub const MAINTENANCE_DRAIN_BUDGET: Duration = Duration::from_millis(1500);

/// Handle that reports when the dispatched signals have all been consumed.
///
/// Detached execution alone cannot promise durability: a one-shot process can
/// exit before the hook has written anything, so the caller needs a way to know
/// the work landed. The handle offers a bounded wait, and dropping it keeps the
/// old fire-and-forget behaviour.
#[derive(Debug)]
pub struct DispatchJob {
    done: Receiver<()>,
}

impl DispatchJob {
    /// Wait up to `budget` for the hook to consume every dispatched signal.
    ///
    /// `true` when the hook finished inside the budget. A panicking hook drops
    /// its sender, which returns `false` immediately rather than blocking.
    #[must_use]
    pub fn drain(self, budget: Duration) -> bool {
        self.done.recv_timeout(budget).is_ok()
    }
}

/// Deliver leaf-attached signals on a detached thread so consolidation never blocks.
///
/// Returns immediately, with a handle a caller may [`DispatchJob::drain`] when
/// it needs the signals' own effects to have landed before it answers. If no
/// signals were produced, no thread is spawned. Delivery order matches
/// `signals`. A slow or panicking hook affects only its own detached thread.
///
/// `#[must_use]`: dropping the handle is the fire-and-forget path, honest only
/// for a process that outlives the hook. A caller that answers a write must
/// drain the handle and record the verdict it gets.
#[must_use]
pub fn dispatch_async(
    hook: SharedMaintenanceHook,
    signals: Vec<MaintenanceSignal>,
) -> Option<DispatchJob> {
    if signals.is_empty() {
        return None;
    }
    let (done_tx, done) = channel();
    std::thread::spawn(move || {
        for signal in signals {
            hook.on_leaf_attached(signal);
        }
        let _ = done_tx.send(());
    });
    Some(DispatchJob { done })
}

/// No-op hook when no maintenance subscriber is present.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMaintenance;

impl MaintenanceHook for NoopMaintenance {
    fn on_leaf_attached(&self, _signal: MaintenanceSignal) {}
}

/// Buffers signals for tests and early hosts.
#[derive(Debug, Default)]
pub struct BufferMaintenance {
    signals: Mutex<Vec<MaintenanceSignal>>,
}

impl BufferMaintenance {
    /// Snapshot of buffered signals.
    #[must_use]
    pub fn signals(&self) -> Vec<MaintenanceSignal> {
        self.signals.lock().expect("maintenance buffer").clone()
    }
}

impl MaintenanceHook for BufferMaintenance {
    fn on_leaf_attached(&self, signal: MaintenanceSignal) {
        self.signals
            .lock()
            .expect("maintenance buffer")
            .push(signal);
    }
}

/// In-process leaf façade: queryable immediately after gated admit.
#[derive(Debug, Default)]
pub struct LeafIndex {
    items: Mutex<HashMap<(String, String), MemoryItem>>,
    time_to_queryable_ms: Mutex<HashMap<String, u128>>,
}

impl LeafIndex {
    /// Empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a leaf as queryable (`leaf_queryable=true` equivalent).
    pub fn publish(&self, item: MemoryItem) {
        let key = (item.bank_id.clone(), item.id.clone());
        self.items.lock().expect("leaf index").insert(key, item);
    }

    /// Record the elapsed time through the latest leaf-publish boundary.
    pub fn record_time_to_queryable(&self, bank_id: &str, time_to_queryable_ms: u128) {
        self.time_to_queryable_ms
            .lock()
            .expect("queryable timing")
            .insert(bank_id.to_owned(), time_to_queryable_ms);
    }

    /// Read the latest leaf-publish timing for a bank.
    #[must_use]
    pub fn time_to_queryable_ms(&self, bank_id: &str) -> u128 {
        self.time_to_queryable_ms
            .lock()
            .expect("queryable timing")
            .get(bank_id)
            .copied()
            .unwrap_or(0)
    }

    /// Get by bank + id (does not require clean ancestors).
    #[must_use]
    pub fn get(&self, bank_id: &str, item_id: &str) -> Option<MemoryItem> {
        self.items
            .lock()
            .expect("leaf index")
            .get(&(bank_id.to_owned(), item_id.to_owned()))
            .cloned()
    }

    /// Drop a leaf from the in-memory index (compliance erasure).
    ///
    /// Returns `true` when a leaf was present. Used so shredded plaintext does
    /// not linger in the process-local leaf cache.
    pub fn remove(&self, bank_id: &str, item_id: &str) -> bool {
        self.items
            .lock()
            .expect("leaf index")
            .remove(&(bank_id.to_owned(), item_id.to_owned()))
            .is_some()
    }

    /// Time-ordered leaf list for a bank (by `created_at`, then id).
    #[must_use]
    pub fn list_leaves(&self, bank_id: &str) -> Vec<MemoryItem> {
        let guard = self.items.lock().expect("leaf index");
        let mut out: Vec<_> = guard
            .values()
            .filter(|i| i.bank_id == bank_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod maintenance_tests;
