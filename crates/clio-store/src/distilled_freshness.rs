// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one distilled unit's read-time freshness: its
//! proof count, the time its current revision was reconciled, and the single
//! stale state with its reason, computed from store-supplied timestamps only.
//!
//! ## Owns
//! - `ClaimedSourceState`, the metadata read of one claimed source
//! - `UnitFreshness`, the freshness answer every read surface renders
//! - `compute_freshness`, the pure comparison over a record and its sources
//! - `unit_freshness`, the one store-calling composition
//! - `stamp_millis`, the store's portable stamp reader
//!
//! ## Does not own
//! - The record and its evidence list (`distilled_unit`, `distilled_evidence`)
//! - The claim-state SQL (`sqlite_distilled_unit` / `postgres_distilled_unit`)
//! - Rendering on a read surface (`clio-mcp`) and the wave report (`clio-write`)
//! - The recall eligibility rule (`clio-retrieve`)
//!
//! ## Boundary
//!
//! Changes to how staleness is decided should normally require changes here.
//! Changes to the record shape or to the backend SQL should not require changes
//! here.
//!
//! Keep this module focused on deciding freshness, not on reporting it.

use crate::distilled_unit::{DistilledUnitRecord, STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED};
use crate::store::Store;
use clio_types::{days_from_civil, days_in_month};
use std::collections::BTreeMap;

/// One claimed source's read-time state, read without touching content.
///
/// `updated_at` is `None` when the row is gone; `active` is false once the item
/// left active use through `discard`, so one read answers both halves of the
/// staleness question and never decrypts a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedSourceState {
    /// Item id of the claimed source.
    pub source_id: String,
    /// The source's last-change time, absent when the row is gone.
    pub updated_at: Option<String>,
    /// Whether the source is still in active use.
    pub active: bool,
}

/// One unit's freshness, as the store can tell it on a read.
///
/// `stale` is three-state on purpose: `Some(false)` is a unit whose sources all
/// predate its last reconciliation, `Some(true)` names the reason it is not
/// current, and `None` says the store could not tell — a stamp it cannot read is
/// reported as unknown rather than as current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFreshness {
    /// Distinct sources in the current revision's evidence list.
    pub proof_count: usize,
    /// When the current revision was reconciled (the record's own stamp).
    pub last_reconciled_at: String,
    /// `None` when no stamp could be compared.
    pub stale: Option<bool>,
    /// The reason, present exactly when `stale` is `Some(true)`.
    pub stale_reason: Option<String>,
}

impl UnitFreshness {
    /// A stale answer with `reason`.
    #[must_use]
    fn stale(reason: &str, proof_count: usize, last_reconciled_at: &str) -> Self {
        Self {
            proof_count,
            last_reconciled_at: last_reconciled_at.to_owned(),
            stale: Some(true),
            stale_reason: Some(reason.to_owned()),
        }
    }
}

/// Decide one unit's freshness from its record and its sources' read-time state.
///
/// Three outcomes, in this order of precedence:
///
/// 1. A stale state the store already recorded (the operations-removal cascade,
///    or a refused revision) is reported as it stands. A read does not
///    second-guess a determination that was written down.
/// 2. A claimed source that is no longer in active use makes the unit stale for
///    `source_removed`: that is an instruction that the content leave active
///    use, and it outranks a mere edit elsewhere in the claim set.
/// 3. A claimed source whose last change is newer than the reconciliation makes
///    the unit stale for `source_changed`.
///
/// A source the caller did not report, or whose stamp cannot be read, leaves the
/// answer unknown (`stale` = `None`) rather than current.
#[must_use]
pub fn compute_freshness(
    record: &DistilledUnitRecord,
    states: &[ClaimedSourceState],
) -> UnitFreshness {
    let proof_count = record.proof_count();
    let reconciled_at = record.distilled_at.as_str();
    if let Some(reason) = &record.stale_reason {
        return UnitFreshness::stale(reason, proof_count, reconciled_at);
    }
    let Some(reconciled) = stamp_millis(reconciled_at) else {
        return UnitFreshness {
            proof_count,
            last_reconciled_at: reconciled_at.to_owned(),
            stale: None,
            stale_reason: None,
        };
    };
    let by_id: BTreeMap<&str, &ClaimedSourceState> = states
        .iter()
        .map(|state| (state.source_id.as_str(), state))
        .collect();
    let mut readable = true;
    let mut removed = false;
    let mut changed = false;
    for id in record.claim_ids() {
        let Some(state) = by_id.get(id.as_str()) else {
            readable = false;
            continue;
        };
        if !state.active {
            removed = true;
            continue;
        }
        match state.updated_at.as_deref().and_then(stamp_millis) {
            Some(changed_at) if changed_at > reconciled => changed = true,
            Some(_) => {}
            None => readable = false,
        }
    }
    if removed {
        return UnitFreshness::stale(STALE_SOURCE_REMOVED, proof_count, reconciled_at);
    }
    if changed {
        return UnitFreshness::stale(STALE_SOURCE_CHANGED, proof_count, reconciled_at);
    }
    UnitFreshness {
        proof_count,
        last_reconciled_at: reconciled_at.to_owned(),
        stale: readable.then_some(false),
        stale_reason: None,
    }
}

/// Read one unit's freshness: its record's own state plus one metadata read of
/// its claimed sources.
///
/// # Errors
/// Storage failures reading the claim states.
pub fn unit_freshness(
    store: &dyn Store,
    bank_id: &str,
    record: &DistilledUnitRecord,
) -> Result<UnitFreshness, clio_types::AmError> {
    let claim: Vec<String> = record.claim_ids().into_iter().collect();
    let states = store.distilled_unit_claim_state(bank_id, &claim)?;
    Ok(compute_freshness(record, &states))
}

/// A store timestamp as epoch milliseconds, or `None` when it is not a stamp
/// this store writes.
///
/// Two forms, because two writers stamp items in this store: epoch milliseconds
/// (the maintenance jobs, via `migrate::now_iso`) and
/// `YYYY-MM-DD[Thh:mm:ss[.fff]][Z]` (the MCP write path). The comparison has to
/// survive both, so both are converted to one number here instead of compared as
/// text — a lexicographic compare of the two forms is not a comparison of times.
///
/// Narrow on purpose: an offset zone, a spelled month, a day the month does not
/// have, or anything else is `None`, so a stamp this reader cannot place yields
/// an unknown freshness rather than a wrong "current". Every range is checked
/// for existence, not only for width: the conversion this reader hands the date
/// to does not validate it, so a day of 31 in a 30-day month would otherwise
/// become a real instant in the following month.
pub(crate) fn stamp_millis(raw: &str) -> Option<i64> {
    let stamp = raw.trim();
    if stamp.is_empty() {
        return None;
    }
    // Epoch millis are all digits (optionally signed), and that is the whole
    // test: an ISO date also starts with a digit, so the branch cannot be on the
    // first character. A `-` only ever starts a negative epoch, since no date
    // form here carries a sign.
    if stamp
        .strip_prefix('-')
        .unwrap_or(stamp)
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        return stamp.parse::<i64>().ok();
    }
    iso_millis(stamp)
}

/// `YYYY-MM-DD[Thh:mm:ss[.fff]][Z]` → epoch milliseconds, UTC.
fn iso_millis(stamp: &str) -> Option<i64> {
    if stamp.len() < 10 || stamp.as_bytes()[4] != b'-' || stamp.as_bytes()[7] != b'-' {
        return None;
    }
    let year: i64 = stamp.get(0..4)?.parse().ok()?;
    let month: u32 = stamp.get(5..7)?.parse().ok()?;
    let day: u32 = stamp.get(8..10)?.parse().ok()?;
    // The day is checked against the length of the month it names, not against a
    // fixed 31. A width check alone lets `2025-02-29` through, and the conversion
    // below does not validate: it turns an impossible day into a real instant in
    // the next month, so a freshness decision would be made against a time the
    // writer never wrote. An unplaceable date is `None` instead.
    if !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    let mut millis = days_from_civil(year, month, day).checked_mul(86_400_000)?;
    if stamp.len() == 10 {
        return Some(millis);
    }
    if stamp.as_bytes()[10] != b'T' && stamp.as_bytes()[10] != b' ' {
        return None;
    }
    let rest = stamp.get(11..)?;
    let rest = rest.strip_suffix('Z').unwrap_or(rest);
    let (clock, fraction) = match rest.split_once('.') {
        Some((clock, digits)) => {
            let millis_digits: String = digits.chars().take(3).collect();
            if millis_digits.is_empty() || !millis_digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            (clock, format!("{millis_digits:0<3}").parse::<i64>().ok()?)
        }
        None => (rest, 0),
    };
    if clock.len() != 8 || clock.as_bytes()[2] != b':' || clock.as_bytes()[5] != b':' {
        return None;
    }
    let hour: i64 = clock.get(0..2)?.parse().ok()?;
    let minute: i64 = clock.get(3..5)?.parse().ok()?;
    let second: i64 = clock.get(6..8)?.parse().ok()?;
    // `second == 60` is refused, and the range is checked as 0..=59 rather than
    // 0..=60. RFC 3339 does admit a 60th second, but only as the last second of
    // June or December in a year that actually had a leap second, and nothing
    // this reader consumes carries one: the two writers in this store stamp
    // whole seconds from a clock that never reports 60. The reader cannot check
    // the position without a leap-second table, so accepting 60 anywhere would
    // place a stamp the writer never wrote. It stays `None` — the same unknown
    // this function returns for every other stamp it cannot place.
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    millis = millis.checked_add(hour * 3_600_000 + minute * 60_000 + second * 1_000 + fraction)?;
    Some(millis)
}

#[cfg(test)]
#[path = "distilled_freshness_tests.rs"]
mod distilled_freshness_tests;

#[cfg(test)]
#[path = "distilled_freshness_shape_tests.rs"]
mod distilled_freshness_shape_tests;

#[cfg(test)]
#[path = "distilled_freshness_stamp_tests.rs"]
mod distilled_freshness_stamp_tests;
