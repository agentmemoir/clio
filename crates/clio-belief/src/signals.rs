// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for five-factor signals on the belief **create** path.
//!
//! ## Owns
//! - `BeliefSignalSource` (neighbor / recency lookups for a proposition)
//! - Belief `SignalInputs` construction and gate-free scoring
//!
//! ## Does not own
//! - Factor formulas or threshold policy (`clio-admission`)
//! - Belief persistence (`clio-store`) or tool orchestration (`observe`)
//!
//! ## Boundary
//!
//! Changes to how belief propositions score on create should normally require changes here.
//! Changes to persistence or tool wiring should not require changes here.
//!
//! Keep this module focused on belief create-path signals.

use clio_admission::{AdmissionPolicy, Decision, SignalInputs, decide_ungated, factor_unavailable};
use clio_types::{AmError, ErrorCode, SourceType};

/// Threshold/prior key for the belief-writes row.
///
/// Beliefs have no semantic-category gate; this key selects the type prior
/// and `θ_admit` override for a new belief identity. The key is accepted by
/// `clio-config` admission-knob validation; with no override, the prior defaults
/// to 0.5 and `θ_admit` falls back to the global threshold.
pub const BELIEF_KEY: &str = "belief";

/// Neighbor / recency lookups needed to score a new belief identity.
///
/// Hosts supply the adapter; `clio-belief` ships no default store adapter.
pub trait BeliefSignalSource: std::fmt::Debug {
    /// Neighbor texts for the novelty signal.
    ///
    /// # Errors
    /// `factor_unavailable` when the store backend is down.
    fn neighbor_texts(&self, bank_id: &str, proposition: &str) -> Result<Vec<String>, AmError>;

    /// Age of the nearest prior in days; `None` means no prior exists.
    ///
    /// # Errors
    /// `factor_unavailable` when the store backend is down.
    fn prior_age_days(&self, bank_id: &str, proposition: &str) -> Result<Option<f64>, AmError>;
}

/// Keep an existing `factor_unavailable`, else wrap the backend failure.
fn as_factor_unavailable(e: AmError) -> AmError {
    if e.code == ErrorCode::FactorUnavailable {
        return e;
    }
    factor_unavailable(&e.message)
}

/// Build scoring inputs for a proposition from its own text plus store lookups.
///
/// # Errors
/// `factor_unavailable` when a lookup backend is down.
pub fn signals_for(
    bank_id: &str,
    proposition: &str,
    source_type: SourceType,
    source: &dyn BeliefSignalSource,
) -> Result<SignalInputs, AmError> {
    let neighbors = source
        .neighbor_texts(bank_id, proposition)
        .map_err(as_factor_unavailable)?;
    let prior_age_days = source
        .prior_age_days(bank_id, proposition)
        .map_err(as_factor_unavailable)?;
    Ok(SignalInputs {
        text: proposition.to_owned(),
        source_type: Some(source_type),
        neighbors: Some(neighbors),
        prior_age_days,
        recency_unavailable: false,
    })
}

/// Score a new belief identity with five factors and **no category gate**.
///
/// # Errors
/// `factor_unavailable` when a required signal backend is down.
pub fn score_new_belief(
    bank_id: &str,
    proposition: &str,
    source_type: SourceType,
    source: &dyn BeliefSignalSource,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    let signals = signals_for(bank_id, proposition, source_type, source)?;
    decide_ungated(BELIEF_KEY, &signals, policy)
}
