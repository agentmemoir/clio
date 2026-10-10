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
//! - `LeafAttached` maintenance signal buffer for later dirty-path consumers
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
use std::sync::{Arc, Mutex};

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
/// Contract: the write path MUST NOT wait on this hook. `parallel_ingest` delivers
/// signals on a detached dispatcher thread (see [`dispatch_async`]), so an
/// implementor may do heavy work without gating the write response. Implementors
/// MUST be `Send + Sync` so they can be shared with that dispatcher.
pub trait MaintenanceHook: std::fmt::Debug {
    /// Notify that a leaf is durable and queryable; path may be dirty.
    fn on_leaf_attached(&self, signal: MaintenanceSignal);
}

/// Thread-safe handle to a maintenance hook for non-blocking dispatch.
pub type SharedMaintenanceHook = Arc<dyn MaintenanceHook + Send + Sync>;

/// Deliver leaf-attached signals on a detached thread so ingest never blocks.
///
/// Returns immediately. If no signals were produced, no thread is spawned.
/// Delivery order matches `signals`. A slow or panicking hook affects only its
/// own detached thread, never the write path.
pub fn dispatch_async(hook: SharedMaintenanceHook, signals: Vec<MaintenanceSignal>) {
    if signals.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for signal in signals {
            hook.on_leaf_attached(signal);
        }
    });
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
