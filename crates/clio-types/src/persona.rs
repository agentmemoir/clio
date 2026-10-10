// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for persona-document shapes and their
//! structural validation.
//!
//! ## Owns
//! - `PersonaStableEntry`, `PersonaPreference`, `PersonaDocument` shapes
//! - Validation of `admission_score` / `confidence` ranges and required fields
//! - The optional `context` field on persona records (surfaced by `persona_get`;
//!   sealed at rest by the store, never truncated here)
//!
//! ## Does not own
//! - Persistence, encryption, admission scoring, or budget truncation
//! - EMA math (`attribute`) or belief objects (`belief`)
//!
//! ## Boundary
//!
//! Changes to persona field shapes or validation rules should normally require
//! changes here. Changes to persona storage or tool wiring should not require
//! changes here.
//!
//! Keep this module focused on persona-document shapes and validation.

use crate::error::AmError;
use crate::item::validate_context;
use serde::{Deserialize, Serialize};

/// Provenance of one persona entry (vocabulary: `source`, never the belief
/// provenance enum `source_type`).
pub const PERSONA_SOURCES: [&str; 2] = ["explicit", "inferred"];

/// One durable discrete/categorical persona attribute (invalidation semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaStableEntry {
    /// Bank isolation key.
    #[serde(skip)]
    pub bank_id: String,
    /// Stable attribute key (e.g. `communication_style`); unique open row per bank.
    pub key: String,
    /// Categorical value (never a scalar intensity; EMA payloads are rejected).
    pub value: String,
    /// Optional confidence in `0.0..=1.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Entry provenance: `explicit` | `inferred` (NOT `source_type`).
    pub source: String,
    /// Evidence reference (turn id / tool call id); `None` when unrecorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Optional free-text rationale; sealed at rest under the bank DEK and
    /// surfaced as plaintext by `persona_get` (omit the field when unrecorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Admission score stamped at the last gated write, in `0.0..=1.0`.
    pub admission_score: f64,
    /// Valid-time start (ISO-8601 text).
    pub as_of: String,
    /// Valid-time end; `None` = currently open (visible in the document).
    #[serde(skip)]
    pub valid_until: Option<String>,
}

/// One continuous/scalar persona preference (EMA state + trend).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaPreference {
    /// Bank isolation key.
    #[serde(skip)]
    pub bank_id: String,
    /// Stable attribute key (e.g. `brevity_affinity`).
    pub key: String,
    /// Current EMA state (numeric only at rest).
    pub value: f64,
    /// Slower trend baseline from the shared engine.
    pub trend: f64,
    /// Optional confidence in `0.0..=1.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Entry provenance: `explicit` | `inferred` (NOT `source_type`).
    pub source: String,
    /// Evidence reference (turn id / tool call id); `None` when unrecorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Optional free-text rationale; sealed at rest under the bank DEK and
    /// surfaced as plaintext by `persona_get` (omit the field when unrecorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Create-time admission score, retained across observes, in `0.0..=1.0`.
    pub admission_score: f64,
    /// Observation timestamp of the applied observation.
    pub as_of: String,
}

/// The bounded always-on companion object.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PersonaDocument {
    /// Durable discrete/categorical entries.
    #[serde(default)]
    pub stable: Vec<PersonaStableEntry>,
    /// Continuous/scalar intensities with `trend`.
    #[serde(default)]
    pub preferences: Vec<PersonaPreference>,
}

/// Validate a persona provenance label.
///
/// # Errors
/// Empty or unknown source label.
pub fn validate_persona_source(source: &str) -> Result<(), AmError> {
    if PERSONA_SOURCES.contains(&source) {
        return Ok(());
    }
    Err(AmError::new(
        crate::error::ErrorCode::InvalidArgument,
        format!("persona source must be explicit|inferred, got `{source}`"),
    ))
}

fn check_score(label: &str, score: f64) -> Result<(), AmError> {
    if score.is_finite() && (0.0..=1.0).contains(&score) {
        return Ok(());
    }
    Err(AmError::new(
        crate::error::ErrorCode::OutOfRange,
        format!("{label} must be in 0.0..=1.0"),
    ))
}

fn check_confidence(confidence: Option<f64>) -> Result<(), AmError> {
    match confidence {
        Some(c) if c.is_finite() && (0.0..=1.0).contains(&c) => Ok(()),
        Some(_) => Err(AmError::new(
            crate::error::ErrorCode::OutOfRange,
            "confidence must be in 0.0..=1.0",
        )),
        None => Ok(()),
    }
}

impl PersonaStableEntry {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Empty key/value/source, out-of-range score/confidence, or a `context`
    /// that is empty or over the published byte maximum.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.key.is_empty() || self.value.is_empty() {
            return Err(AmError::new(
                crate::error::ErrorCode::InvalidArgument,
                "persona stable entry requires key and value",
            ));
        }
        if self.as_of.is_empty() {
            return Err(AmError::new(
                crate::error::ErrorCode::InvalidArgument,
                "persona stable entry requires as_of",
            ));
        }
        validate_persona_source(&self.source)?;
        check_score("admission_score", self.admission_score)?;
        check_confidence(self.confidence)?;
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        Ok(())
    }
}

impl PersonaPreference {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Empty key/source, non-finite value, out-of-range score/confidence, or a
    /// `context` that is empty or over the published byte maximum.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.key.is_empty() {
            return Err(AmError::new(
                crate::error::ErrorCode::InvalidArgument,
                "persona preference requires key",
            ));
        }
        if self.as_of.is_empty() {
            return Err(AmError::new(
                crate::error::ErrorCode::InvalidArgument,
                "persona preference requires as_of",
            ));
        }
        if !self.value.is_finite() {
            return Err(AmError::new(
                crate::error::ErrorCode::OutOfRange,
                "persona preference value must be a finite number",
            ));
        }
        if !self.trend.is_finite() {
            return Err(AmError::new(
                crate::error::ErrorCode::OutOfRange,
                "persona preference trend must be a finite number",
            ));
        }
        validate_persona_source(&self.source)?;
        check_score("admission_score", self.admission_score)?;
        check_confidence(self.confidence)?;
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        Ok(())
    }
}

impl PersonaDocument {
    /// Validate every entry.
    ///
    /// # Errors
    /// Any invalid stable entry or preference.
    pub fn validate(&self) -> Result<(), AmError> {
        for entry in &self.stable {
            entry.validate()?;
        }
        for pref in &self.preferences {
            pref.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "persona_tests.rs"]
mod persona_tests;
