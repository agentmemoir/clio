// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the observing `Embedder` wrapper the index
//! coordinator runs rebuild sweeps through, so rebuild latency and errors keep
//! feeding the rolling worker state the status surface reads.
//!
//! ## Owns
//! - `ObservingEmbedder`: the `Embedder` decorator used by
//!   `IndexCoordinator::rebuild_indexes`
//!
//! ## Does not own
//! - The coordinator, its drain, or its rolling state (`worker`)
//! - Embedding transport (`embed` / `http`)
//!
//! ## Boundary
//!
//! Changes to how rebuild embeds are observed should normally require changes
//! here. Changes to drain or rebuild policy should not.
//!
//! Keep this module focused on the observing wrapper.

use crate::embed::Embedder;
use crate::worker::WorkerState;
use clio_types::AmError;
use std::sync::Mutex;
use std::time::Instant;

/// [`Embedder`] wrapper that records sweep embed latency/error into the
/// coordinator's rolling worker state, preserving the observability
/// contract (`status().embed.last_latency_ms` / `last_error`) for rebuilds.
pub(crate) struct ObservingEmbedder<'a> {
    inner: &'a dyn Embedder,
    state: &'a Mutex<WorkerState>,
}

impl<'a> ObservingEmbedder<'a> {
    /// Wrap `inner`, recording each embed into the coordinator's worker state.
    pub(crate) fn new(inner: &'a dyn Embedder, state: &'a Mutex<WorkerState>) -> Self {
        Self { inner, state }
    }
}

impl Embedder for ObservingEmbedder<'_> {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        let t0 = Instant::now();
        let vectors = self.inner.embed_batch(texts);
        match &vectors {
            Ok(_) => {
                let mut state = self.state.lock().expect("index coordinator state poisoned");
                state.last_latency_ms = Some(t0.elapsed().as_millis());
            }
            Err(e) => {
                let mut state = self.state.lock().expect("index coordinator state poisoned");
                state.last_error = Some(e.to_string());
            }
        }
        vectors
    }

    fn health(&self) -> Result<(), AmError> {
        self.inner.health()
    }

    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    fn dims(&self) -> usize {
        self.inner.dims()
    }
}
