// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the last-writer-wins comparison the apply
//! matrix runs against a locally held row: reading an origin receipt's
//! admission score, deciding whether the incoming mutation loses, and
//! recording that loss in the conflict journal.
//!
//! ## Owns
//! - `lww_loser`: the incoming-versus-existing decision and its conflict row
//! - `admission_score`: the tolerant receipt reader both sides use
//!
//! ## Does not own
//! - The comparison rule itself (`protocol::lww_compare`)
//! - The apply matrix and its per-kind effects (`apply`, `apply_items`,
//!   `apply_records`, `apply_history`)
//! - Conflict storage (the `clio-store` sync backend)
//!
//! ## Boundary
//!
//! Changes to how a lost comparison is recorded should normally require
//! changes here. Changes to which row wins should not: that rule lives in
//! `protocol`.
//!
//! Keep this module focused on recording a lost LWW comparison.

use crate::apply::ApplyEngine;
use crate::clock::now_utc;
use crate::protocol::SyncMutation;
use clio_store::SyncConflictRow;
use clio_types::AmError;

/// The admission score one origin receipt carries, read tolerantly: a missing
/// or non-numeric `score` yields `None`, which `lww_compare` maps to `0.0`.
/// Shared by the conflict comparison and the pull-side receipt restoration, so
/// both read the same key the same way and a malformed value degrades to the
/// documented fallback rather than to an error.
pub(crate) fn admission_score(receipt: &serde_json::Value) -> Option<f64> {
    receipt.get("score").and_then(serde_json::Value::as_f64)
}

/// True when the incoming mutation loses to the locally held row, in which
/// case the loss is recorded in the conflict journal first.
///
/// # Errors
/// Storage failures.
pub(crate) fn lww_loser(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    existing_time: &str,
    existing_score: Option<f64>,
) -> Result<bool, AmError> {
    let incoming_score = m.admission_receipt.as_ref().and_then(admission_score);
    // The stored row does not carry its origin device id, so the local node's
    // device id stands in as the existing side of the tie-break. That removes
    // the old empty-string asymmetry where any remote device always won a
    // full tie. Locally held rows are the common case; a peer-origin row is
    // resolved from this node's perspective consistently.
    let incoming_wins = crate::protocol::lww_compare(
        &m.transaction_time,
        incoming_score,
        &m.device_id,
        existing_time,
        existing_score,
        &engine.device_id,
    ) == std::cmp::Ordering::Greater;
    if incoming_wins {
        return Ok(false);
    }
    engine.sync.conflict_insert(&SyncConflictRow {
        bank_id: m.bank.clone(),
        entity_kind: m.entity_kind.clone(),
        entity_id: m.entity_id.clone(),
        winner_mutation_id: format!("local:{}", m.entity_id),
        loser_mutation_id: m.sync_mutation_id.clone(),
        winner_transaction_time: existing_time.to_owned(),
        loser_transaction_time: m.transaction_time.clone(),
        winner_device_id: engine.device_id.clone(),
        loser_device_id: m.device_id.clone(),
        created_at: now_utc(),
    })?;
    Ok(true)
}

#[cfg(test)]
#[path = "apply_lww_tests.rs"]
mod tests;
