// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the runtime index drain: the bounded outbox
//! sweep, its cadence bounds, and the PII-safe pipeline snapshot.
//!
//! ## Owns
//! - `IndexDrain`: a bounded `drain` entry plus `status` over one store backend
//! - `INDEX_MAX_BATCH` / `INDEX_SWEEP_INTERVAL` runtime cadence bounds
//!
//! ## Does not own
//! - Outbox processing semantics (`clio_index::IndexCoordinator`)
//! - Provider resolution or MCP tool dispatch (see `ops_embedder` / `runtime`)
//!
//! ## Boundary
//!
//! Changes to runtime drain batching or cadence should normally require changes
//! here. Changes to outbox processing policy should not require changes here.
//!
//! Keep this module focused on the runtime drain wrapper.
//!
//! # Why the drain lives here
//!
//! The store writes the `index_pending` outbox transactionally, so the write
//! response never waits on embedding. This wrapper runs the existing
//! `IndexCoordinator` off the response path in bounded batches; a lexical-only
//! coordinator is used when no embedder is configured, so a no-provider install
//! still indexes lexically instead of leaving retrieval silently empty.

use clio_index::{DEFAULT_MAX_ATTEMPTS, Embedder, IndexCoordinator, IndexStatus, ProcessReport};
use clio_store::{IndexStore, Store};
use clio_types::AmError;
use std::sync::Arc;
use std::time::Duration;

/// Max outbox jobs drained per tick (documented backpressure bound).
pub const INDEX_MAX_BATCH: usize = clio_index::DEFAULT_JOB_BATCH;
/// Periodic sweep cadence; a post-write nudge drains sooner than this.
pub const INDEX_SWEEP_INTERVAL: Duration = Duration::from_millis(500);

/// Bounded, testable drain over the durable index outbox.
pub struct IndexDrain {
    coordinator: IndexCoordinator,
    batch: usize,
}

impl IndexDrain {
    /// Build a drain; `None` selects the lexical-only coordinator so a
    /// no-embedder install still writes lexical docs.
    ///
    /// # Errors
    /// Embedder dims mismatch against the active space, or storage failures.
    pub fn new(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        embedder: Option<Arc<dyn Embedder>>,
        batch: usize,
    ) -> Result<Self, AmError> {
        let coordinator = match embedder {
            Some(embedder) => IndexCoordinator::new(store, index, embedder, DEFAULT_MAX_ATTEMPTS)?,
            None => IndexCoordinator::new_lexical_only(store, index, DEFAULT_MAX_ATTEMPTS)?,
        };
        Ok(Self {
            coordinator,
            batch: batch.max(1),
        })
    }

    /// `true` when a dense embedder is configured for this drain.
    #[must_use]
    pub fn dense_available(&self) -> bool {
        self.coordinator.dense_available()
    }

    /// Max jobs processed per `drain` call.
    #[must_use]
    pub fn batch_size(&self) -> usize {
        self.batch
    }

    /// Process one bounded batch of due jobs without blocking any response.
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn drain(&self) -> Result<ProcessReport, AmError> {
        self.coordinator.process_pending(self.batch)
    }

    /// PII-safe pipeline snapshot for the requested scope.
    ///
    /// # Errors
    /// Coverage query failures.
    pub fn status(&self, bank: Option<&str>) -> Result<IndexStatus, AmError> {
        self.coordinator.index_status(bank)
    }
}

#[cfg(test)]
#[path = "index_drain_tests.rs"]
mod index_drain_tests;

#[cfg(test)]
#[path = "index_drain_hardening_tests.rs"]
mod index_drain_hardening_tests;
