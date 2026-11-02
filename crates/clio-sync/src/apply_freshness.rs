// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two envelope-order guards of the sync
//! apply engine: refusing a stamp that sits outside a configured wall-clock
//! bound, and refusing a replay at or below the newest stamp already applied
//! from the same device.
//!
//! ## Owns
//! - `guard`: the future-skew check and the per-device high-water check, in
//!   that order, and the dead-letter write each one produces
//! - `record`: raising the per-device high-water mark after a durable apply
//!
//! ## Does not own
//! - The age bounds and the stamp reader (`freshness`)
//! - Dedupe on `sync_mutation_id`, the content-hash guard, the denial guards,
//!   and the op matrix (`apply`)
//! - Reading the clock (`apply` passes one `now` text per apply)
//! - Storing the mark (the `clio-store` sync backend)
//!
//! ## Boundary
//!
//! Changes to which envelopes are refused should normally require changes
//! here. Changes to what an apply does with an accepted envelope should not.
//!
//! Keep this module focused on envelope-order refusal.

use crate::apply::apply_dead_letter::dead_letter_with_reason;
use crate::apply::{ApplyEngine, ApplyOutcome, ApplyResult};
use crate::freshness::{Freshness, parse_utc_secs};
use crate::protocol::{REASON_STALE_ENVELOPE, SyncMutation};
use clio_types::AmError;

/// Refuse an envelope that is ahead of the clock or strictly older than the
/// device mark.
///
/// Each wall-clock half runs only when its bound is configured. Both ship off
/// by default, because a legitimate stamp is not always a change time: a triple
/// ships a transaction-time or valid-time close, and a non-upsert item
/// operation ships a stored transaction or valid time, so a row can carry a
/// world-time stamp years ahead. The past bound is off because a legitimate
/// backlog can be arbitrarily old.
///
/// The ordering half is the defence: a stamp strictly older than the newest one
/// already applied from the same device is a replay, whatever its age.
///
/// An equal stamp is accepted, deliberately. `transaction_time` has second
/// granularity, so two different mutations from one device can carry the same
/// stamp — two entities changed in the same second, or a batch packaged in one
/// pass. Refusing an equal stamp would dead-letter real traffic and freeze the
/// bank's pull cursor. The mark is not a unique key: an equal stamp is ordered
/// by the protocol's last-writer-wins comparison, and an exact replay is still
/// deduplicated by the mutation-id journal.
///
/// Text comparison is deliberate. The wire carries `transaction_time` as
/// text, and the protocol already orders stamps lexicographically through
/// `lww_compare`, so the mark inherits the same ordering.
///
/// The mark is per `(bank, device, entity kind)` because the stamp encoding
/// varies by kind, not by device: one device writes ISO-8601 seconds for some
/// kinds, a bare date for others, and epoch milliseconds for the rows
/// `clio-store` stamps itself. Comparing across kinds would compare three
/// encodings as text, where a millisecond stamp sorts before every ISO stamp
/// and a legitimate row would be refused. Keeping the mark per kind compares
/// only stamps written in one encoding.
///
/// An unparsable `now` skips the wall-clock halves rather than refusing: the
/// shape of `transaction_time` is the envelope check's business, not this
/// one's.
///
/// # Errors
/// Storage failures.
pub(crate) fn guard(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    now: &str,
) -> Result<Option<ApplyResult>, AmError> {
    if let Some(now_secs) = parse_utc_secs(now) {
        // `classify_text` consults each bound only when it is non-zero, so a
        // disabled side can never produce its refusal here.
        match engine.policy.classify_text(&m.transaction_time, now_secs) {
            Freshness::Future => {
                return refuse(
                    engine,
                    m,
                    envelope_json,
                    "transaction_time is ahead of the local clock",
                );
            }
            Freshness::Stale => {
                return refuse(
                    engine,
                    m,
                    envelope_json,
                    "transaction_time is older than the configured age bound",
                );
            }
            Freshness::Fresh => {}
        }
    }
    if let Some(mark) = engine
        .sync
        .device_high_water_get(&m.bank, &m.device_id, &m.entity_kind)?
        && m.transaction_time.as_str() < mark.as_str()
    {
        return refuse(
            engine,
            m,
            envelope_json,
            "transaction_time is older than the newest stamp applied from this device",
        );
    }
    Ok(None)
}

/// Raise the per-device high-water mark to this mutation's stamp.
///
/// Call it only once the row is known to be applied and journaled, so a
/// refused or duplicate delivery never moves the mark. The stored mark only
/// moves forward: an equal or older stamp leaves it unchanged.
///
/// # Errors
/// Storage failures.
pub(crate) fn record(engine: &ApplyEngine<'_>, m: &SyncMutation) -> Result<(), AmError> {
    engine
        .sync
        .device_high_water_set(&m.bank, &m.device_id, &m.entity_kind, &m.transaction_time)
}

/// Dead-letter one refused envelope and report it as a durable reject.
///
/// The reason is passed explicitly: the mapped path would fall through to
/// `malformed` and mislabel the row.
fn refuse(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    detail: &str,
) -> Result<Option<ApplyResult>, AmError> {
    let outcome = dead_letter_with_reason(engine, m, envelope_json, REASON_STALE_ENVELOPE, detail)?;
    debug_assert!(matches!(outcome, ApplyOutcome::DeadLettered { .. }));
    Ok(Some(ApplyResult::plain(outcome)))
}

#[cfg(test)]
#[path = "apply_freshness_tests.rs"]
mod tests;
