// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `belief_history` read tool and its
//! point-in-time (`as_of`) view.
//!
//! ## Owns
//! - Identity resolution by belief id or proposition
//! - The ordered trajectory response shape
//!
//! ## Does not own
//! - Belief persistence (`clio-store`) or observe writes (`observe`)
//!
//! ## Boundary
//!
//! Changes to trajectory reads should normally require changes here.
//! Changes to belief writes should not require changes here.
//!
//! Keep this module focused on belief trajectory reads.

use clio_store::BeliefStore;
use clio_types::{AmError, ConfidenceEntry, ErrorCode, SourceType, proposition_key};
use serde::{Deserialize, Serialize};

/// Full inspectable belief trajectory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeliefHistory {
    /// Stable belief id.
    pub belief_id: String,
    /// Proposition text (exact, never a gist substitute).
    pub proposition: String,
    /// Ordered entries; filtered to `as_of <= T` when a point-in-time is given.
    pub entries: Vec<ConfidenceEntry>,
    /// Confidence current at the requested point (last entry of `entries`).
    pub current_confidence: Option<f64>,
    /// Provenance current at the requested point.
    pub current_source_type: Option<SourceType>,
    /// Optional bounded descriptive source context omitted when the
    /// belief was written without context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Echo of the point-in-time filter.
    pub as_of: Option<String>,
}

/// Read a belief's trajectory by id or by proposition.
///
/// Identity resolution tries the exact id first, then the normalized
/// proposition key. Unknown ids/propositions return `ErrorCode::NotFound`.
///
/// `as_of` is compared against stored entry `as_of` text (lexicographic ISO-8601
/// ordering); pass the same canonical form the entries use (see
/// `clio_types::ConfidenceEntry::as_of`).
///
/// # Errors
/// Missing bank, invalid target, unknown belief (`NotFound`), crypto, or storage
/// failures.
pub fn belief_history(
    store: &dyn BeliefStore,
    bank_id: &str,
    belief_id_or_proposition: &str,
    as_of: Option<&str>,
) -> Result<BeliefHistory, AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if belief_id_or_proposition.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "belief id or proposition is required",
        ));
    }
    let belief = if let Some(belief) = store.get_belief(bank_id, belief_id_or_proposition)? {
        belief
    } else {
        let key = proposition_key(belief_id_or_proposition);
        store
            .find_belief_by_proposition(bank_id, &key)?
            .ok_or_else(|| not_found(belief_id_or_proposition))?
    };
    let entries = if as_of.is_some() {
        store.belief_history(bank_id, &belief.id, as_of)?
    } else {
        belief.confidence_history.clone()
    };
    Ok(BeliefHistory {
        belief_id: belief.id,
        proposition: belief.proposition,
        current_confidence: entries.last().map(|e| e.confidence),
        current_source_type: entries.last().map(|e| e.source_type),
        context: belief.context,
        entries,
        as_of: as_of.map(str::to_owned),
    })
}

fn not_found(target: &str) -> AmError {
    AmError::new(ErrorCode::NotFound, format!("belief not found: `{target}`"))
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;
