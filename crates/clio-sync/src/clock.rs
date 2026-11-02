// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for wall-clock stamping for the sync crate:
//! the UTC text form used for journal/created timestamps and DLQ records, the
//! epoch-second read of the local clock, and the reader that puts a written
//! stamp back into epoch seconds.
//!
//! ## Owns
//! - `now_utc()` epoch → ISO-8601 UTC text conversion (same format as the
//!   store's durable stamps)
//! - `now_epoch()`: one read of the local clock as epoch seconds
//! - `parse_utc_epoch()`: strict `YYYY-MM-DDTHH:MM:SSZ` → epoch seconds
//!
//! ## Does not own
//! - Transaction-time semantics (callers pass writer stamps on envelopes)
//! - Cursor or LWW ordering logic (see `protocol`)
//! - The calendar arithmetic itself (`clio-types` `civil` owns it)
//!
//! ## Boundary
//!
//! Changes to the sync stamp format should normally require changes here and
//! in `clio-store` together. Changes to calendar arithmetic should not require
//! changes here. Keep this module focused on clock stamping.

use clio_types::{civil_from_days, days_from_civil, days_in_month};

/// Current UTC time as ISO-8601 text (comparable with the store's stamps).
#[must_use]
pub fn now_utc() -> String {
    format_utc(now_epoch())
}

/// Epoch seconds → the `YYYY-MM-DDTHH:MM:SSZ` text the writer stamps.
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Current UTC time in epoch seconds.
///
/// A clock read before the Unix epoch yields `0` rather than a panic, which is
/// the fallback a machine with a broken clock gets instead of a stamp it cannot
/// place. A `u64` second count too large for `i64` clamps to `i64::MAX`; no real
/// clock reaches it.
#[must_use]
pub(crate) fn now_epoch() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
}

/// `YYYY-MM-DDTHH:MM:SSZ` → epoch seconds, UTC.
///
/// The shape is the one `now_utc()` writes: fixed width, no fraction, no
/// offset, `Z` last. Anything else is `None`, including trailing text, a
/// missing `Z`, and a day the month does not have. The date is validated
/// against the length of the month it names, because `days_from_civil` does
/// not check it: an impossible day would otherwise become a real instant in
/// the next month.
#[must_use]
#[allow(
    dead_code,
    reason = "called by this module's tests until the freshness read uses it"
)]
pub(crate) fn parse_utc_epoch(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 20 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return None;
        }
    }
    let year: i64 = stamp.get(0..4)?.parse().ok()?;
    let month: u32 = stamp.get(5..7)?.parse().ok()?;
    let day: u32 = stamp.get(8..10)?.parse().ok()?;
    if !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    let hour: i64 = stamp.get(11..13)?.parse().ok()?;
    let minute: i64 = stamp.get(14..16)?.parse().ok()?;
    let second: i64 = stamp.get(17..19)?.parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod clock_tests;
