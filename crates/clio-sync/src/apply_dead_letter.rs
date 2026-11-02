// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync dead-letter quarantine helpers of
//! the apply engine: operator-forced quarantine, reason-code mapped quarantine
//! of permanently unapplicable mutations, and the operator-visible record of a
//! stale-envelope refusal.
//!
//! ## Owns
//! - `quarantine` (operator-facing permanent reject, e.g. unusable local key)
//! - `dead_letter` (reason-code mapped quarantine for apply rejects)
//! - `quarantine_stale`, the dead letter and status record of a stale refusal
//! - The `stale_refused` counter and the `__status` last-refusal line
//!
//! ## Does not own
//! - The apply matrix and LWW conflict resolution (`apply`)
//! - Cursor advance policy (`client`) or dead-letter row persistence
//!   (the `clio-store` sync backend)
//! - The freshness decision itself (`apply_freshness`)
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
use crate::protocol::{REASON_STALE_ENVELOPE, SyncMutation, reason_code_for};
use clio_store::{DeadLetterStatus, SyncDeadLetterRow, SyncStore};
use clio_types::AmError;

/// The per-bank cursor label that holds the monotonic stale-refusal count.
pub(crate) const STALE_REFUSED_KEY: &str = "stale_refused";

/// The `__status` slot key that holds the last stale refusal's line, beside the
/// `last_error` line that slot already carries.
pub(crate) const LAST_STALE_REFUSAL_KEY: &str = "last_stale_refusal";

/// The bank id of the node-wide status slot.
const STATUS_SLOT: &str = "__status";

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

/// Quarantine one stale push and record the refusal where an operator can see
/// it: a dead-letter row under [`REASON_STALE_ENVELOPE`], one increment of the
/// bank's monotonic `stale_refused` count, and the last-refusal line in the
/// node-wide status slot.
///
/// A stale row writes no durable content, so without this record the sender's
/// watermark could move past a row the receiver dropped and nothing would show
/// it. The row lands in `bank_id`, the bank the push was authorized for, so a
/// refusal never writes into another bank.
///
/// # Errors
/// Storage failures.
pub(crate) fn quarantine_stale(
    engine: &ApplyEngine<'_>,
    bank_id: &str,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let at = now_utc();
    let detail = format!(
        "refused stale envelope: writer stamp {} is outside this node's freshness bounds at {at}",
        m.transaction_time
    );
    let outcome = quarantine(
        engine,
        bank_id,
        m,
        envelope_json,
        REASON_STALE_ENVELOPE,
        &detail,
    )?;
    record_stale_refusal(engine.sync, bank_id, m, &at)?;
    Ok(outcome)
}

/// Bump the bank's refusal count and write the last-refusal line. The count is
/// read-add-write through the existing cursor slot, so it only ever grows and
/// an unrelated cursor write leaves it alone.
fn record_stale_refusal(
    sync: &dyn SyncStore,
    bank_id: &str,
    m: &SyncMutation,
    at: &str,
) -> Result<(), AmError> {
    let next = stale_refused(sync, bank_id)?.saturating_add(1);
    sync.cursor_set(bank_id, STALE_REFUSED_KEY, &next.to_string())?;
    sync.cursor_set(
        STATUS_SLOT,
        LAST_STALE_REFUSAL_KEY,
        &format!(
            "{at} {REASON_STALE_ENVELOPE}: refused mutation {} carrying writer stamp {}",
            m.sync_mutation_id, m.transaction_time
        ),
    )
}

/// How many stale envelopes this node has refused for `bank_id` (`0` when it
/// has refused none).
///
/// # Errors
/// Storage failures.
pub(crate) fn stale_refused(sync: &dyn SyncStore, bank_id: &str) -> Result<i64, AmError> {
    Ok(sync
        .cursor_get(bank_id, STALE_REFUSED_KEY)?
        .and_then(|count| count.parse().ok())
        .unwrap_or(0))
}

/// The last stale refusal this node recorded, if any.
///
/// # Errors
/// Storage failures.
pub(crate) fn last_stale_refusal(sync: &dyn SyncStore) -> Result<Option<String>, AmError> {
    sync.cursor_get(STATUS_SLOT, LAST_STALE_REFUSAL_KEY)
}
