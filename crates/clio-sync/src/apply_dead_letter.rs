// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync dead-letter quarantine helpers of
//! the apply engine: operator-forced quarantine and reason-code mapped
//! quarantine of permanently unapplicable mutations.
//!
//! ## Owns
//! - `quarantine` (operator-facing permanent reject, e.g. unusable local key)
//! - `dead_letter` (reason-code mapped quarantine for apply rejects)
//!
//! ## Does not own
//! - The apply matrix and LWW conflict resolution (`apply`)
//! - Cursor advance policy (`client`) or dead-letter row persistence
//!   (the `clio-store` sync backend)
//!
//! ## Boundary
//!
//! Changes to the quarantine row shape or reason-code mapping should normally
//! require changes here. Changes to the apply matrix should not require
//! changes here.
//!
//! Keep this module focused on sync dead-letter quarantine helpers.

use crate::apply::{ApplyEngine, ApplyOutcome};
use crate::clock::now_utc;
use crate::protocol::{SyncMutation, reason_code_for};
use clio_store::{DeadLetterStatus, SyncDeadLetterRow};
use clio_types::AmError;

/// Quarantine one mutation as a permanent reject (operator-facing helper for
/// transports that must freeze cursors on unusable rows, e.g. missing key).
///
/// The row lands in `bank_id`, which for a transport refusal is the bank the
/// request was already authorized for, so a refusal never writes into the
/// bank it refused.
///
/// # Errors
/// Storage failures.
pub fn quarantine(
    engine: &ApplyEngine<'_>,
    bank_id: &str,
    m: &SyncMutation,
    envelope_json: &str,
    reason_code: &str,
    detail: &str,
) -> Result<ApplyOutcome, AmError> {
    // Per-credential scoping makes a bank a tenant boundary, so a refusal must
    // not write rows into a bank the credential cannot reach: `bank_id` is
    // inside the credential's own scope, so the row stays visible to the
    // credential that caused it and to the operator, and nothing lands in the
    // other tenant's bank.
    engine.sync.dead_letter_open(&SyncDeadLetterRow {
        sync_mutation_id: m.sync_mutation_id.clone(),
        bank_id: bank_id.to_owned(),
        reason_code: reason_code.to_owned(),
        detail: detail.to_owned(),
        envelope_json: envelope_json.to_owned(),
        status: DeadLetterStatus::Open,
        first_seen: crate::clock::now_utc(),
        last_error: None,
        updated_at: None,
    })?;
    Ok(ApplyOutcome::DeadLettered {
        reason_code: reason_code.to_owned(),
    })
}

/// Dead-letter one mutation with the mapped reason code (shared with the
/// domain-record apply effects in `apply_records`).
pub(crate) fn dead_letter(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    err: &AmError,
) -> Result<ApplyOutcome, AmError> {
    let reason = reason_code_for(err);
    engine.sync.dead_letter_open(&SyncDeadLetterRow {
        sync_mutation_id: m.sync_mutation_id.clone(),
        bank_id: m.bank.clone(),
        reason_code: reason.to_owned(),
        detail: err.message.clone(),
        envelope_json: envelope_json.to_owned(),
        status: DeadLetterStatus::Open,
        first_seen: now_utc(),
        last_error: None,
        updated_at: None,
    })?;
    Ok(ApplyOutcome::DeadLettered {
        reason_code: reason.to_owned(),
    })
}
