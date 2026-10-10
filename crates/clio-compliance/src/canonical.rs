// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the single-slot canonical memory interface
//! Canonical key naming and `canonical_get` reads over
//! subject-keyed item rows.
//!
//! ## Owns
//! - `canonical_subject` single-slot key naming (`canonical:{key}`)
//! - `canonical_get` active / point-in-time (`as_of`) read
//!
//! ## Does not own
//! - `canonical_put` commit sequencing: the atomic executor resolves the prior
//!   slot row in-transaction and stages close+create+audit as
//!   `clio_store::BatchWrite::CanonicalPut` (see `batch_sqlite` / `batch_postgres`)
//! - Category gate + five-factor scoring (caller gates before commit;
//!   a rejected value must never supersede the prior one)
//! - Not to be confused with write-pipeline canonical consolidation
//!
//! ## Boundary
//!
//! Changes to canonical key naming or canonical reads should normally require
//! changes here. Changes to MCP binding or admission math should not.
//!
//! Keep this module focused on the canonical slot read adapter.

use clio_store::Store;
use clio_types::{AmError, ErrorCode};
use serde::Serialize;

/// Single-slot subject id for a canonical key.
#[must_use]
pub fn canonical_subject(key: &str) -> String {
    format!("canonical:{key}")
}

/// Currently active canonical value (bi-temporal metadata included).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CanonicalValue {
    /// Canonical key.
    pub key: String,
    /// Item id holding the current value.
    pub item_id: String,
    /// Value prose (the item's non-authoritative gist).
    pub value: String,
    /// Authoritative structured snapshot, when the item carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<serde_json::Value>,
    /// Valid-time start of the current revision.
    pub valid_from: String,
    /// Transaction-time start of the current revision.
    pub tx_from: String,
}

/// Read the currently active (or `as_of` historical) value of a canonical slot.
///
/// # Errors
/// Storage failures.
pub fn canonical_get(
    store: &dyn Store,
    bank_id: &str,
    key: &str,
    as_of: Option<&str>,
) -> Result<Option<CanonicalValue>, AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if key.trim().is_empty() {
        return Err(AmError::new(ErrorCode::InvalidArgument, "key is required"));
    }
    let item = store.get_item_by_subject(bank_id, &canonical_subject(key), as_of)?;
    Ok(item.map(|item| CanonicalValue {
        key: key.to_owned(),
        item_id: item.id,
        value: item.gist.clone().unwrap_or_default(),
        snapshot: item.snapshot,
        valid_from: item.valid_from,
        tx_from: item.tx_from,
    }))
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod canonical_tests;
