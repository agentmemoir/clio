// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-belief` crate root: belief objects, append-only
//! confidence trajectories, and the `belief_observe` / `belief_history` tools.
//!
//! ## Owns
//! - Public module graph and re-exports for belief writes and reads
//! - The frozen `RetrieveHit` response contract re-export for consumers
//!
//! ## Does not own
//! - Belief persistence drivers (`clio-store`) or scoring math (`clio-admission`)
//! - Hybrid retrieval, compose, or MCP transport bindings
//!
//! ## Boundary
//!
//! Changes to the belief tool surface should normally require changes here.
//! Changes to storage drivers or ranking math should not require changes here.
//!
//! Keep this module focused on the belief tool surface.

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]

pub mod history;
pub mod observe;
pub mod signals;

pub use history::{BeliefHistory, belief_history};
pub use observe::{
    BeliefEvent, BeliefObserveArgs, BeliefObserveOutcome, BeliefSink, BeliefVecSink, belief_observe,
};
pub use signals::{BELIEF_KEY, BeliefSignalSource, score_new_belief, signals_for};

pub use clio_types::{BeliefObject, ConfidenceEntry, RetrieveHit, SourceType, proposition_key};
