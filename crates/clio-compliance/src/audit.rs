// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `audit_trail` service entrypoint:
//! assembling the lifecycle of one item from write-path structures and
//! masking secret-shaped values before the trail leaves the service layer.
//!
//! ## Owns
//! - `audit_trail` facade over `Store::audit_trail`
//! - `AuditTrailView` masked JSON payload shape
//! - Inline secret scrubbing of the revision source `context`
//! - `ITEM_NOT_FOUND` mapping for missing ids
//!
//! ## Does not own
//! - Backend SQL and lineage traversal (`clio-store`)
//! - MCP result shaping (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to trail masking or the view shape should normally require changes
//! here. Changes to storage SQL should not require changes here.
//!
//! Keep this module focused on the audit trail service.

use clio_config::secret::{mask_value, scrub_inline_secrets};
use clio_store::{ItemAuditTrail, Store};
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Masked audit-trail payload.
///
/// Never carries decrypted item content, DEKs, or secret-shaped values; the
/// masking pass is applied defensively over the whole view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditTrailView {
    /// Item id the caller queried.
    pub item_id: String,
    /// Full lineage, ordered by transaction-time start.
    pub lineage_ids: Vec<String>,
    /// One revision row per lineage id (masked JSON).
    pub revisions: Vec<Value>,
    /// Telemetry events across the lineage, ordered ascending.
    pub events: Vec<Value>,
    /// Belief confidence trajectory, ordered ascending (empty for facts).
    pub belief_confidence: Vec<Value>,
    /// Revisions, telemetry events, and belief points merged into ONE sequence,
    /// ordered ascending by `(timestamp, kind, id)` ("ordered array of
    /// every change"). Each row is `{kind, timestamp, entry}`.
    pub timeline: Vec<Value>,
}

/// Reconstruct the full lifecycle of `item_id` in `bank_id`.
///
/// A read-side view over bi-temporal revisions, telemetry rows, and belief
/// confidence history — never a second logging store. Secret-shaped values in
/// the trail (including correction reasons) are masked.
///
/// # Errors
/// `item_not_found` when the id is absent from the caller's bank, or store
/// failures.
pub fn audit_trail(
    store: &dyn Store,
    bank_id: &str,
    item_id: &str,
) -> Result<AuditTrailView, AmError> {
    if bank_id.is_empty() || item_id.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "bank_id and item_id are required",
        ));
    }
    let trail: ItemAuditTrail = store.audit_trail(bank_id, item_id)?;
    let mut view = AuditTrailView {
        item_id: trail.item_id,
        lineage_ids: trail.lineage_ids,
        revisions: trail
            .revisions
            .iter()
            .map(|r| serde_json::to_value(r).expect("revision serializes"))
            .collect(),
        events: trail
            .events
            .iter()
            .map(|e| serde_json::to_value(e).expect("entry serializes"))
            .collect(),
        belief_confidence: trail
            .belief_confidence
            .iter()
            .map(|p| serde_json::to_value(p).expect("point serializes"))
            .collect(),
        timeline: Vec::new(),
    };
    for event in &mut view.events {
        mask_value("", event);
    }
    for revision in &mut view.revisions {
        mask_value("", revision);
        scrub_context_secrets(revision);
    }
    for point in &mut view.belief_confidence {
        mask_value("", point);
    }
    // Merge the (already masked) arrays into one ascending sequence.
    let timeline = build_timeline(&view);
    view.timeline = timeline;
    Ok(view)
}

/// Inline-scrub the descriptive source `context` of one revision.
///
/// [`mask_value`] masks values under secret-looking *keys*; `context` is not
/// one, so a secret pasted into the text must be removed by shape before the
/// trail leaves the service. Runs before [`build_timeline`], so the merged
/// timeline carries the same scrubbed revision.
fn scrub_context_secrets(revision: &mut Value) {
    if let Some(Value::String(context)) = revision.get_mut("context") {
        *context = scrub_inline_secrets(context);
    }
}

/// Interleave revisions, telemetry events, and belief points into one
/// deterministically ordered timeline keyed by `(timestamp, kind, id)`.
fn build_timeline(view: &AuditTrailView) -> Vec<Value> {
    let mut rows: Vec<(String, String, String, Value)> = Vec::new();
    for revision in &view.revisions {
        rows.push((
            field_str(revision, "tx_from"),
            "revision".to_owned(),
            field_str(revision, "item_id"),
            revision.clone(),
        ));
    }
    for event in &view.events {
        rows.push((
            field_str(event, "timestamp"),
            "event".to_owned(),
            field_str(event, "item_id"),
            event.clone(),
        ));
    }
    for point in &view.belief_confidence {
        rows.push((
            field_str(point, "as_of"),
            "belief".to_owned(),
            field_str(point, "belief_id"),
            point.clone(),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    rows.into_iter()
        .map(|(timestamp, kind, _id, entry)| {
            json!({"kind": kind, "timestamp": timestamp, "entry": entry})
        })
        .collect()
}

/// String field of a JSON object (empty when absent/non-string).
fn field_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
#[path = "audit_context_tests.rs"]
mod audit_context_tests;

#[cfg(test)]
#[path = "audit_tests.rs"]
mod audit_tests;
