// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live ranking environment a retriever
//! reads: the one `RwLock<RankingEnv>` store and the consistent snapshot a
//! request takes from it before any work.
//!
//! ## Owns
//! - `LiveRanking`, the lock, its whole-env and weights-only setters, and the
//!   request-scoped snapshot
//! - `LiveSnapshot`, the one consistent copy of the retrieval weights,
//!   retention half-life, and trust ratios a request reads
//!
//! ## Does not own
//! - Which stage reads each knob or how a request uses it (`hybrid`,
//!   `hybrid_rank`)
//! - Ranking-env defaults and validation (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to how live ranking knobs are stored, replaced, or snapshotted
//! should normally require changes here. Changes to retrieval stages that read
//! the knobs should not require changes here.
//!
//! Keep this module focused on one consistent snapshot of the live knobs.

use clio_config::ranking::{RankingEnv, RetrievalWeights, TrustRatios};
use std::sync::RwLock;

/// One consistent copy of the live ranking knobs a request needs.
///
/// Cloned out in a single read-lock scope, so a request sees one knob set for
/// its whole lifetime and the guard is dropped before any work or I/O.
#[derive(Debug, Clone)]
pub(crate) struct LiveSnapshot {
    /// Retrieval leg weights plus the importance weight.
    pub(crate) weights: RetrievalWeights,
    /// Retention half-life in days for the age factor.
    pub(crate) half_life_days: f64,
    /// Trust ratios for the trust multiplier.
    pub(crate) trust: TrustRatios,
}

/// The live ranking environment of one retriever.
///
/// A setter may run while retrieval calls are in flight. Every request takes
/// one snapshot through [`Self::snapshot`] and reads only the copy, so a change
/// made during a request lands on the next request and one request never mixes
/// two knob sets. The read path never takes the write lock.
#[derive(Debug)]
pub(crate) struct LiveRanking {
    env: RwLock<RankingEnv>,
}

impl LiveRanking {
    /// Start from the system-default ranking environment.
    pub(crate) fn new() -> Self {
        Self {
            env: RwLock::new(RankingEnv::system_defaults()),
        }
    }

    /// Replace the whole environment for requests started after this call.
    pub(crate) fn set_env(&self, env: &RankingEnv) {
        *self
            .env
            .write()
            .expect("retriever ranking env lock poisoned") = env.clone();
    }

    /// Replace only the retrieval-weight half, leaving the other knobs alone.
    pub(crate) fn set_weights(&self, weights: RetrievalWeights) {
        self.env
            .write()
            .expect("retriever ranking env lock poisoned")
            .retrieval = weights;
    }

    /// The current retrieval weights.
    pub(crate) fn weights(&self) -> RetrievalWeights {
        self.env
            .read()
            .expect("retriever ranking env lock poisoned")
            .retrieval
            .clone()
    }

    /// Copy the three request-scoped knobs in one read-lock scope.
    pub(crate) fn snapshot(&self) -> LiveSnapshot {
        let env = self
            .env
            .read()
            .expect("retriever ranking env lock poisoned");
        LiveSnapshot {
            weights: env.retrieval.clone(),
            half_life_days: env.retention.half_life_days,
            trust: env.trust,
        }
    }
}

#[cfg(test)]
#[path = "live_ranking_tests.rs"]
mod live_ranking_tests;
