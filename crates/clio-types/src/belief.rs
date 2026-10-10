// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for belief objects and their append-only
//! confidence trajectories.
//!
//! ## Owns
//! - `ConfidenceEntry` and `BeliefObject` shapes and well-formedness
//! - The documented proposition identity normalization (`proposition_key`)
//!
//! ## Does not own
//! - Persistence, encryption, admission scoring, or telemetry
//! - Fact/episodic item shapes (see `item`) or triple intervals (see `triple`)
//!
//! ## Boundary
//!
//! Changes to belief field shapes and identity rules should normally require changes here.
//! Changes to belief persistence should not require changes here.
//!
//! Keep this module focused on belief object shapes and identity.

use crate::error::{AmError, ErrorCode};
use crate::item::SourceType;
use serde::{Deserialize, Serialize};

/// One observation appended to a belief's confidence trajectory.
///
/// `as_of` values are stored and compared as ISO-8601 text: ordering and
/// `as_of` filters (`as_of <= T`) are lexicographic. Use ONE canonical form per
/// belief — date-only (`2026-06-01`) or full timestamp (`2026-06-01T12:00:00Z`)
/// — and never mix the two within one trajectory, or same-day boundaries become
/// ambiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceEntry {
    /// Observation time (ISO-8601 text; string-sortable; see format note above).
    pub as_of: String,
    /// Observed confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Evidence reference (turn id / tool call id).
    pub evidence_ref: String,
    /// How this piece of evidence arrived.
    pub source_type: SourceType,
}

impl ConfidenceEntry {
    /// Build a validated entry.
    ///
    /// # Errors
    /// Empty `as_of` / `evidence_ref`, or `confidence` outside `0.0..=1.0`.
    pub fn new(
        as_of: impl Into<String>,
        confidence: f64,
        evidence_ref: impl Into<String>,
        source_type: SourceType,
    ) -> Result<Self, AmError> {
        let entry = Self {
            as_of: as_of.into(),
            confidence,
            evidence_ref: evidence_ref.into(),
            source_type,
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Structural well-formedness.
    ///
    /// # Errors
    /// Empty `as_of` / `evidence_ref`, or `confidence` outside `0.0..=1.0`.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.as_of.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "confidence entry as_of is required",
            ));
        }
        if self.evidence_ref.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "confidence entry evidence_ref is required",
            ));
        }
        check_confidence(self.confidence)
    }
}

/// Confidence range guard shared by entries and observe arguments.
///
/// # Errors
/// Non-finite or out-of-range confidence.
pub fn check_confidence(confidence: f64) -> Result<(), AmError> {
    if confidence.is_finite() && (0.0..=1.0).contains(&confidence) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::OutOfRange,
        "confidence must be in 0.0..=1.0",
    ))
}

/// Normalize a proposition into its bank-scoped identity key (documented rule).
///
/// Rule: trim, collapse every whitespace run to one ASCII space, and lowercase.
/// Two propositions that differ only by case or whitespace are the same belief.
/// The key is deterministic so `belief_observe` finds an existing identity and
/// `belief_history(proposition)` resolves it without scanning.
#[must_use]
pub fn proposition_key(proposition: &str) -> String {
    proposition
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A versioned belief object: proposition plus append-only confidence history.
///
/// Under Option B a belief object is implicitly `epistemic_kind = "belief"`:
/// beliefs are NOT items and carry no separate kind field, column, or parameter.
/// The kind is surfaced explicitly on reads via [`crate::read::RetrieveHit`]
/// This struct is the store shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeliefObject {
    /// Stable belief id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Data-subject id for DEK selection.
    pub subject_id: String,
    /// Proposition text (plaintext in memory; encrypted at rest).
    pub proposition: String,
    /// Optional bounded descriptive source context. Plaintext in
    /// memory; sealed under the subject DEK at rest. `None` when the belief
    /// was written without context or the sealed value is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Store-derived effective time of `context`: the store stamps it
    /// at creation (`created_at`) and at each authorized correction, and never
    /// accepts a value from a caller or wire payload. `None` when the belief
    /// carries no context (or on legacy rows written before the stamp existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_as_of: Option<String>,
    /// Append-only trajectory, oldest first.
    pub confidence_history: Vec<ConfidenceEntry>,
    /// Created timestamp.
    pub created_at: String,
    /// Updated timestamp (last append).
    pub updated_at: String,
}

impl BeliefObject {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Empty id/bank/subject/proposition, an invalid history entry, or an
    /// optional `context` that fails [`crate::validate_context`].
    pub fn validate(&self) -> Result<(), AmError> {
        if self.id.is_empty()
            || self.bank_id.is_empty()
            || self.subject_id.is_empty()
            || self.proposition.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "belief id, bank_id, subject_id, and proposition are required",
            ));
        }
        for entry in &self.confidence_history {
            entry.validate()?;
        }
        if let Some(context) = &self.context {
            crate::item::validate_context(context)?;
        }
        Ok(())
    }

    /// Latest history entry, if any.
    #[must_use]
    pub fn current_entry(&self) -> Option<&ConfidenceEntry> {
        self.confidence_history.last()
    }

    /// Current confidence = last entry's confidence.
    #[must_use]
    pub fn current_confidence(&self) -> Option<f64> {
        self.current_entry().map(|e| e.confidence)
    }

    /// Current provenance = last entry's `source_type`.
    #[must_use]
    pub fn current_source_type(&self) -> Option<SourceType> {
        self.current_entry().map(|e| e.source_type)
    }

    /// Point-in-time confidence: latest entry with `as_of <= as_of`.
    ///
    /// `as_of` is compared as text (lexicographic ISO-8601); use the same
    /// canonical form the trajectory's entries use.
    #[must_use]
    pub fn as_of_confidence(&self, as_of: &str) -> Option<f64> {
        self.confidence_history
            .iter()
            .rfind(|e| e.as_of.as_str() <= as_of)
            .map(|e| e.confidence)
    }

    /// Append one validated entry; prior entries are never rewritten.
    ///
    /// # Errors
    /// Invalid entry.
    pub fn append(&mut self, entry: ConfidenceEntry) -> Result<(), AmError> {
        entry.validate()?;
        self.updated_at.clone_from(&entry.as_of);
        self.confidence_history.push(entry);
        Ok(())
    }
}

#[cfg(test)]
#[path = "belief_tests.rs"]
mod belief_tests;
