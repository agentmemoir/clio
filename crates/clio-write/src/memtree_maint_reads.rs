// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance reads a caller can take
//! without running a wave: the coalesce and structural-refresh counters, and
//! the dirty-ancestor count that never waits for the forest lock.
//!
//! ## Owns
//! - The coalesce-hit and structural-refresh timing counters, read on demand
//! - The non-blocking dirty-ancestor count, which reports a held lock as
//!   contention instead of waiting for it
//!
//! ## Does not own
//! - The refresh wave, its in-flight gauge, and the dirty set (`memtree_maint`)
//! - Status response assembly and the tool handlers (`memtree_tools`)
//!
//! ## Boundary
//!
//! Changes to what a caller can learn about a bank's maintenance without
//! running a wave should normally require changes here. Changes to what a wave
//! does should not require changes here.
//!
//! Keep this module focused on maintenance reads that never start work.

use super::MemTreeMaint;
use std::sync::atomic::Ordering;

impl MemTreeMaint {
    /// How many dirty marks hit an already-queued node id.
    #[must_use]
    pub fn coalesce_hits(&self) -> u64 {
        self.stats.coalesce_hits.load(Ordering::Relaxed)
    }

    /// Elapsed time of the latest completed structural refresh wave for a bank.
    ///
    /// Returns zero until a non-empty wave has completed.
    #[must_use]
    pub fn structural_maintenance_ms(&self, bank_id: &str) -> u128 {
        self.last_structural_maintenance_ms
            .lock()
            .expect("maintenance timing")
            .get(bank_id)
            .copied()
            .unwrap_or(0)
    }

    /// How many ancestor nodes are dirty for one bank, read without waiting.
    ///
    /// Non-blocking: it takes the forest lock only while that lock is free, so
    /// a write or a wave already holding it never makes the caller wait. It
    /// never retries, never sleeps, and never waits for a bounded time.
    ///
    /// `None` means the count could not be read: the lock is held by a write or
    /// a wave, or a previous holder panicked and poisoned it. A caller must
    /// treat that as contention, never as zero dirty ancestors, which would
    /// hide real pressure behind a lock it cannot take. `Some(0)` is a real
    /// answer: the lock was free and the bank has no dirty ancestors, or has
    /// no tree at all.
    ///
    /// The count is the one the blocking status read reports for a whole bank:
    /// every node the bank's tree marks dirty, uncapped.
    #[must_use]
    pub fn try_dirty_ancestor_count(&self, bank_id: &str) -> Option<usize> {
        let forest = self.forest.try_lock().ok()?;
        let count = forest
            .tree(bank_id)
            .map_or(0, |tree| tree.dirty_node_ids().len());
        Some(count)
    }
}

#[cfg(test)]
#[path = "memtree_maint_reads_tests.rs"]
mod memtree_maint_reads_tests;
