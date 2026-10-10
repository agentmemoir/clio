// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one distilled unit's revision identity and the
//! item snapshot that projects its derivation record: how a revision's item id
//! is formed, what a revision write requires of its item, and what the item's
//! snapshot carries.
//!
//! ## Owns
//! - `revision_item_id`, the `{unit}:v{n}` id derivation
//! - `validate_revision` / `revision_prior_id`, the revision write's rules
//! - `snapshot_projection` / `projected_unit_id`, the shared record-to-item
//!   projection and its reader
//!
//! ## Does not own
//! - The record shape, its stale state, and its evidence trail (`distilled_unit`,
//!   `distilled_evidence`)
//! - The freshness decision (`distilled_freshness`)
//! - Backend SQL (`sqlite_distilled_unit` / `postgres_distilled_unit`)
//!
//! ## Boundary
//!
//! Changes to revision identity or to the item projection should normally
//! require changes here. Changes to the record's own fields should not require
//! changes here.
//!
//! Keep this module focused on revision identity and the snapshot projection.

use crate::distilled_unit::DistilledUnitRecord;
use clio_types::{AmError, ErrorCode, MemoryItem};

/// Item id of one revision of the unit: the unit id itself at revision 1, then
/// `{unit_id}:v{revision}` for later revisions. The suffix is only formatted,
/// never parsed: the derivation record is the identity authority, and a hub id
/// that happens to contain `:v<n>` only makes longer ids, never a collision.
#[must_use]
pub fn revision_item_id(unit_id: &str, revision: u32) -> String {
    if revision <= 1 {
        unit_id.to_owned()
    } else {
        format!("{unit_id}:v{revision}")
    }
}

/// Validate one revision write against its record: the record must be valid,
/// and the new item must be the record's current revision id in the record's
/// bank, carrying the record's provenance marker and naming the record's hub
/// in its snapshot.
pub(crate) fn validate_revision(
    record: &DistilledUnitRecord,
    new_item: &MemoryItem,
) -> Result<(), AmError> {
    record.validate()?;
    new_item.validate()?;
    if new_item.bank_id != record.bank_id {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit revision does not match the record's bank",
        ));
    }
    let expected = record.current_revision_item_id();
    if new_item.id != expected {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("distilled unit revision item id must be `{expected}`"),
        ));
    }
    if new_item.source_ref.as_deref() != Some(record.marker.as_str()) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit revision marker does not match the item source_ref",
        ));
    }
    let hub = new_item
        .snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|map| map.get("hub_id"))
        .and_then(serde_json::Value::as_str);
    if hub != Some(record.hub_id.as_str()) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit revision snapshot must name the record's hub",
        ));
    }
    Ok(())
}

/// The item id of the record's closed prior revision, for the revision write.
pub(crate) fn revision_prior_id(record: &DistilledUnitRecord) -> Result<String, AmError> {
    record.prior_revision_item_id().ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "a revision write requires revision >= 2",
        )
    })
}

/// The item snapshot projection of a record: the record's own key, the hub id,
/// the source ids, their count, and the stale reason when the unit is not
/// current.
///
/// First distillation (in `clio-write`) and every claim-set/stale update (both
/// backends) build the snapshot from this one function over the same record
/// state, so the record and its snapshot projection cannot drift.
///
/// `unit_id` is what lets a read surface holding only a revision item id
/// (`{unit}:v2`) find the record it belongs to: the record is keyed by the
/// stable unit id, and the record is free to say so because this projection is
/// written from it.
#[must_use]
pub fn snapshot_projection(record: &DistilledUnitRecord) -> serde_json::Value {
    let mut projection = serde_json::json!({
        "unit_id": record.item_id,
        "hub_id": record.hub_id,
        "neighbor_ids": record.source_ids,
        "neighbor_count": record.source_ids.len(),
    });
    if let Some(reason) = &record.stale_reason {
        projection["stale_reason"] = serde_json::Value::String(reason.clone());
    }
    projection
}

/// The record a snapshot projection names, if it names one.
///
/// A snapshot written before this projection carried `unit_id` has no answer
/// here, and a read surface then falls back to treating the item id it was
/// given as the record key.
#[must_use]
pub fn projected_unit_id(snapshot: Option<&serde_json::Value>) -> Option<&str> {
    snapshot
        .and_then(serde_json::Value::as_object)
        .and_then(|map| map.get("unit_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
#[path = "distilled_revision_tests.rs"]
mod distilled_revision_tests;
