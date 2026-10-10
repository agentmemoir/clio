// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hygiene persistence contract:
//! reversible hygiene flags, the archive exclusion marker, and the durable
//! hygiene audit log.
//!
//! ## Owns
//! - `HygieneStore` trait and the [`HygieneLogEntry`] row type
//! - The per-item flag/archive/unarchive transaction contract (backend SQL per
//!   driver, each writing its own `audit_events` row)
//!
//! ## Does not own
//! - Noise scoring, candidate ranking, or clean orchestration (`clio-hygiene`)
//! - Compliance erasure; hygiene never touches DEKs or `tombstones`
//! - MCP argument decoding or masking (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to which rows the hygiene transactions write should normally require
//! changes here and in the per-backend `*_hygiene` modules. Changes to the
//! hygiene tool workflow should not require changes here.
//!
//! Keep this module focused on the hygiene persistence contract.

use crate::model::ItemIdentity;
use clio_types::AmError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One durable hygiene audit-log row.
///
/// The store persists one row per item per clean action; a multi-item clean
/// shares one `batch_id` (in `detail`) across its rows so the invocation stays
/// reconstructible. `detail` carries ids, scores, and outcome labels only —
/// never secret material or decrypted content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HygieneLogEntry {
    /// Store-assigned row id (`hlg-<batch>-<index>-<item>`; the index keeps
    /// duplicate target item ids collision-free).
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Item the action was applied to.
    pub item_id: String,
    /// Noise score at audit time, when the caller supplied one.
    pub noise_score: Option<f64>,
    /// Applied action: `flag` | `archive` | `discard` | `keep` | `delete`.
    pub action: String,
    /// Attribution actor.
    pub actor: String,
    /// `confirm=true` was present on the clean call.
    pub confirmed: bool,
    /// Dry-run report row (zero mutations).
    pub dry_run: bool,
    /// Outcome payload (ids, scores, reasons — secret-safe by construction).
    pub detail: Value,
    /// Row creation timestamp.
    pub created_at: String,
}

/// Persistence contract for the hygiene path.
///
/// Implementations MUST record an `audit_events` row in the same
/// transaction as every flag/archive/unarchive mutation, and MUST NOT destroy
/// DEKs or write compliance tombstones.
pub trait HygieneStore: Send + Sync {
    /// Set (or, with `None`, clear) the review-only hygiene flag on one item.
    ///
    /// Flagged items stay fully retrievable; the flag is review metadata only.
    ///
    /// # Errors
    /// Missing bank/item, not-found, or storage failures.
    fn set_hygiene_flag(
        &self,
        bank_id: &str,
        item_id: &str,
        note: Option<&str>,
        actor: &str,
        timestamp: &str,
    ) -> Result<ItemIdentity, AmError>;

    /// Mark one item archived: excluded from default retrieve, readable by id.
    ///
    /// Reversible via [`HygieneStore::unarchive_item`]. Rejects discarded and
    /// already-archived items as `InvalidArgument`, unknown items as `NotFound`.
    ///
    /// # Errors
    /// Missing ids, not-found, already-archived, or storage failures.
    fn archive_item(
        &self,
        bank_id: &str,
        item_id: &str,
        archived_at: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError>;

    /// Clear the archive marker, restoring default-retrieve eligibility.
    ///
    /// # Errors
    /// Missing ids, not-found, not-archived, or storage failures.
    fn unarchive_item(
        &self,
        bank_id: &str,
        item_id: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError>;

    /// Append one durable hygiene audit-log row.
    ///
    /// # Errors
    /// Storage failures or invalid action labels.
    fn insert_hygiene_log(&self, entry: &HygieneLogEntry) -> Result<(), AmError>;

    /// Read hygiene log rows, newest first (bank filter optional).
    ///
    /// # Errors
    /// Zero limit, out-of-range cap, or storage failures.
    fn list_hygiene_logs(
        &self,
        bank_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<HygieneLogEntry>, AmError>;
}

/// Validate the `hygiene_audit_logs.action` CHECK domain before writing.
pub(crate) fn validate_log_action(action: &str) -> Result<(), AmError> {
    match action {
        "flag" | "archive" | "discard" | "keep" | "delete" => Ok(()),
        other => Err(clio_types::AmError::new(
            clio_types::ErrorCode::InvalidArgument,
            format!("hygiene action must be flag|archive|discard|keep|delete, got `{other}`"),
        )),
    }
}
