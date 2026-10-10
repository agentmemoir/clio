// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the audit-trail and correction data models
//! shared by both storage backends.
//!
//! ## Owns
//! - `CorrectionSpec` / `CorrectionOutcome` (atomic discrete correction)
//! - `ItemAuditTrail`, `ItemRevision`, `AuditTrailEntry`, and belief
//!   trajectory point shapes returned by `Store::audit_trail`
//!
//! ## Does not own
//! - Backend SQL (each backend module owns its own statements)
//! - Reason masking (`clio-config::secret`) or MCP result shaping
//!
//! ## Boundary
//!
//! Changes to the audit-trail or correction contract should normally require
//! changes here. Changes to one backend's SQL should not require changes here.
//!
//! Keep this module focused on audit and correction data shapes.

use clio_types::MemoryItem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One atomic discrete correction request (state machine).
///
/// The prior record closes on **both** time axes and keeps its create-time
/// admission score untouched (constraints); the corrected
/// assertion is inserted fresh with `corrects_item_id` linking back.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionSpec {
    /// Bank isolation key (must match both item rows).
    pub bank_id: String,
    /// Open prior assertion to supersede.
    pub prior_item_id: String,
    /// Corrected assertion shell (id, content halves, taxonomy copied by the
    /// caller from the prior revision; intervals stamped at the correction).
    pub new_item: MemoryItem,
    /// Mandatory non-empty correction reason (never a `discard_reason`).
    pub reason: String,
    /// Attribution actor.
    pub actor: String,
    /// Correction timestamp: closes the prior axes and stamps the new rows.
    pub timestamp: String,
}

/// Result of one applied discrete correction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionOutcome {
    /// Superseded prior assertion.
    pub prior_item_id: String,
    /// Newly active corrected assertion.
    pub new_item_id: String,
    /// True when the prior was already closed (no-op, no new row).
    pub already_superseded: bool,
}

/// One bi-temporal revision row of a lineage (read-side view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRevision {
    /// Item id.
    pub item_id: String,
    /// Prior assertion this revision corrects (`None` = first revision).
    pub corrects_item_id: Option<String>,
    /// Replacement assertion (`None` = still open).
    pub superseded_by_id: Option<String>,
    /// Valid-time start.
    pub valid_from: String,
    /// Valid-time end (`None` = open).
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end (`None` = current).
    pub tx_until: Option<String>,
    /// Operationally discarded timestamp (`None` = active).
    pub discarded_at: Option<String>,
    /// Semantic category, when semantic.
    pub category: Option<String>,
    /// `fact` or `belief`.
    pub epistemic_kind: String,
    /// Belief confidence at this revision.
    pub confidence: Option<f64>,
    /// Five-factor admission score at creation (never altered by corrections).
    pub admission_score: Option<f64>,
    /// Provenance reference.
    pub source_ref: Option<String>,
    /// Decrypted source `context` at this revision (`None` = absent, or
    /// unreadable once the subject's DEK was destroyed by erasure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// One telemetry event on the reconstructed timeline (event shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditTrailEntry {
    /// Event timestamp.
    pub timestamp: String,
    /// Stored operation label (`store`, `update`, `invalidate`, `discard`,
    /// `correct`, `belief_observe`, ...).
    pub operation: String,
    /// Item the event refers to.
    pub item_id: String,
    /// Semantic category, when recorded.
    pub category: Option<String>,
    /// `fact` | `belief`, when recorded.
    pub epistemic_kind: Option<String>,
    /// Confidence, when recorded.
    pub confidence: Option<f64>,
    /// Admission score, when recorded.
    pub admission_score: Option<f64>,
    /// Attribution actor.
    pub actor: String,
    /// Correction reason, when the event carries one (`correction_reason`).
    pub reason: Option<String>,
    /// Full stored event detail (secret-masked by the audit service).
    pub detail: Value,
}

/// One append-only belief-confidence point (trajectory).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeliefConfidencePoint {
    /// Belief id (item identity).
    pub belief_id: String,
    /// Observation timestamp.
    pub as_of: String,
    /// Recorded confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Evidence reference.
    pub evidence_ref: String,
    /// Provenance source type.
    pub source_type: String,
}

/// Reconstructed lifecycle of one item.
///
/// A read-side view over write-path structures only — no second store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemAuditTrail {
    /// Item id the caller queried.
    pub item_id: String,
    /// Full lineage (backward via `corrects_item_id`, forward via
    /// `superseded_by_id`), ordered by transaction-time start.
    pub lineage_ids: Vec<String>,
    /// One revision row per lineage id (empty for belief targets).
    pub revisions: Vec<ItemRevision>,
    /// Telemetry events across the whole lineage, ordered by
    /// `(timestamp, id)` ascending.
    pub events: Vec<AuditTrailEntry>,
    /// Belief confidence trajectory entries, ordered by `as_of` ascending.
    pub belief_confidence: Vec<BeliefConfidencePoint>,
}

#[cfg(test)]
#[path = "audit_model_audit_correct_tests.rs"]
mod audit_model_audit_correct_tests;

#[cfg(test)]
#[path = "audit_model_correct_reject_tests.rs"]
mod audit_model_correct_reject_tests;

#[cfg(test)]
#[path = "audit_model_subject_guard_tests.rs"]
mod audit_model_subject_guard_tests;
