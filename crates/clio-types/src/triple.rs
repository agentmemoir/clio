// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for bi-temporal SPO triple types and interval algebra.
//!
//! ## Owns
//! - `TripleRecord`, `TimeAxis`, `UpdateRule`, and point-in-time helpers
//! - Half-open `[start, end)` visibility predicate shared by all query paths
//!
//! ## Does not own
//! - Persistence drivers, admission gating, or EMA continuous updates
//!
//! ## Boundary
//!
//! Changes to triple field shapes or as-of interval rules should normally require changes here.
//! Changes to SQL drivers or gated write wiring should not require changes here.

use crate::{AmError, EpistemicKind, ErrorCode, SourceType, validate_context};
use serde::{Deserialize, Serialize};

/// Which bi-temporal axis an `as_of` filter applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeAxis {
    /// World-time validity (`valid_from` / `valid_until`).
    #[default]
    Valid,
    /// System belief / record time (`tx_from` / `tx_until`).
    Transaction,
}

impl TimeAxis {
    /// Stable wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Transaction => "transaction",
        }
    }

    /// Parse from wire/SQL.
    ///
    /// # Errors
    /// Unknown axis.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "valid" => Ok(Self::Valid),
            "transaction" => Ok(Self::Transaction),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("time_axis must be valid|transaction, got `{other}`"),
            )),
        }
    }
}

/// Attribute update mechanism. Never confused with `epistemic_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateRule {
    /// Invalidation / supersession path.
    Discrete,
    /// Continuous EMA path (owned by a later write engine).
    Continuous,
}

impl UpdateRule {
    /// Stable wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discrete => "discrete",
            Self::Continuous => "continuous",
        }
    }

    /// Parse from wire. `fact`/`belief` (the item `epistemic_kind` values) are
    /// rejected with the same structured error — they are not `update_rule`s.
    ///
    /// # Errors
    /// `wrong_update_rule` for anything other than `discrete`/`continuous`.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "discrete" => Ok(Self::Discrete),
            "continuous" => Ok(Self::Continuous),
            other => Err(AmError::new(
                ErrorCode::WrongUpdateRule,
                format!("update_rule must be discrete|continuous, got `{other}`"),
            )),
        }
    }
}

/// Reject continuous/EMA attempts on the discrete triple/invalidation path.
///
/// # Errors
/// `wrong_update_rule` when `Continuous` is requested.
pub fn ensure_discrete_update_rule(rule: Option<UpdateRule>) -> Result<(), AmError> {
    match rule {
        None | Some(UpdateRule::Discrete) => Ok(()),
        Some(UpdateRule::Continuous) => Err(AmError::new(
            ErrorCode::WrongUpdateRule,
            "continuous/EMA updates are not legal on the discrete triple/invalidation path; \
             use the continuous update engine",
        )),
    }
}

/// Half-open interval visibility: `start <= as_of && (end.is_none() || as_of < end)`.
#[must_use]
pub fn interval_contains(start: &str, end: Option<&str>, as_of: &str) -> bool {
    start <= as_of && end.is_none_or(|e| as_of < e)
}

/// Durable SPO edge with bi-temporal intervals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripleRecord {
    /// Stable edge id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Backing episodic item id (gated write carrier).
    pub item_id: String,
    /// SPO subject (not a GDPR `subject_id`).
    pub subject: String,
    /// SPO predicate.
    pub predicate: String,
    /// SPO object payload (not part of supersession identity).
    pub object: String,
    /// Fact vs belief.
    pub epistemic_kind: EpistemicKind,
    /// Required iff belief.
    pub source_type: Option<SourceType>,
    /// Optional confidence in `0.0..=1.0`.
    pub confidence: Option<f64>,
    /// Valid-time start.
    pub valid_from: String,
    /// Valid-time end (open = None).
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end (open = None).
    pub tx_until: Option<String>,
    /// Optional snapshot item ref.
    pub snapshot_ref: Option<String>,
    /// Optional gist item ref.
    pub gist_ref: Option<String>,
    /// Optional bounded descriptive context, opened from the sealed carrier
    /// item (`items.content_ciphertext`) on reads. `None` when the edge was
    /// written without context or the body is unreadable (e.g. after a
    /// crypto-shred). Not persisted as a triple column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Created timestamp.
    pub created_at: String,
    /// Updated timestamp.
    pub updated_at: Option<String>,
}

impl TripleRecord {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Empty fields, violations, inverted intervals, or a `context` that
    /// is empty or over the published byte maximum.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.id.is_empty()
            || self.bank_id.is_empty()
            || self.item_id.is_empty()
            || self.subject.is_empty()
            || self.predicate.is_empty()
            || self.object.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "triple id, bank_id, item_id, subject, predicate, and object are required",
            ));
        }
        if self.valid_from.is_empty() || self.tx_from.is_empty() || self.created_at.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "valid_from, tx_from, and created_at are required",
            ));
        }
        if let Some(end) = &self.valid_until
            && end.as_str() <= self.valid_from.as_str()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "valid_until must be > valid_from (half-open)",
            ));
        }
        if let Some(end) = &self.tx_until
            && end.as_str() <= self.tx_from.as_str()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "tx_until must be > tx_from (half-open)",
            ));
        }
        match self.epistemic_kind {
            EpistemicKind::Belief => {
                if self.source_type.is_none() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "belief triples require source_type",
                    ));
                }
            }
            EpistemicKind::Fact => {
                if self.source_type.is_some() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "fact triples must not include source_type",
                    ));
                }
            }
        }
        if let Some(c) = self.confidence
            && !(0.0..=1.0).contains(&c)
        {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "confidence must be in 0.0..=1.0",
            ));
        }
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        Ok(())
    }

    /// True when this edge is visible on `axis` at `as_of`.
    #[must_use]
    pub fn visible_at(&self, axis: TimeAxis, as_of: &str) -> bool {
        match axis {
            TimeAxis::Valid => {
                interval_contains(&self.valid_from, self.valid_until.as_deref(), as_of)
            }
            TimeAxis::Transaction => {
                interval_contains(&self.tx_from, self.tx_until.as_deref(), as_of)
            }
        }
    }

    /// Open on both axes (eligible for default supersession).
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.valid_until.is_none() && self.tx_until.is_none()
    }
}

/// Pattern + point-in-time filter for `triple_query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleQuery {
    /// Required bank scope.
    pub bank_id: String,
    /// Optional subject constraint.
    pub subject: Option<String>,
    /// Optional predicate constraint.
    pub predicate: Option<String>,
    /// Optional object constraint.
    pub object: Option<String>,
    /// Point-in-time; `None` means “current” (open end on the chosen axis).
    pub as_of: Option<String>,
    /// Axis for `as_of` / current (default valid).
    pub time_axis: TimeAxis,
}

impl TripleQuery {
    /// Validate bank is present.
    ///
    /// # Errors
    /// Missing bank.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.bank_id.is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank_id is required"));
        }
        Ok(())
    }
}

/// Result of an assert/supersede write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleAddOutcome {
    /// Newly inserted edge id.
    pub new_edge_id: String,
    /// Prior open edges closed by supersession (empty when none).
    pub prior_edge_ids: Vec<String>,
}

/// Result of `triple_end` / item `invalidate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleEndOutcome {
    /// Edges or items closed this call.
    pub closed_ids: Vec<String>,
    /// True when targets were already closed (documented no-op success).
    pub already_ended: bool,
    /// Optional replacement linkage for telemetry.
    pub replacement_id: Option<String>,
}

#[cfg(test)]
#[path = "triple_tests.rs"]
mod triple_tests;
