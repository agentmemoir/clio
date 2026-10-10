// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one UTC civil-calendar conversion shared
//! by every crate: days since the Unix epoch to `(year, month, day)` and back.
//!
//! ## Owns
//! - `civil_from_days` / `days_from_civil`: Howard Hinnant's
//!   proleptic-Gregorian conversions in both directions, the single copy of that
//!   arithmetic in the workspace
//! - `days_in_month` / `is_leap_year`: the month lengths and leap rule, so a
//!   caller placing a date validates it against the same calendar
//!
//! ## Does not own
//! - Clock reads or RFC 3339 timestamp formatting and parsing (each caller keeps
//!   its own `now_utc` and its own stamp reader)
//! - The columns and fields the resulting stamps are written to
//!
//! ## Boundary
//!
//! Changes to the calendar arithmetic should normally require changes here.
//! Callers must call these functions instead of copying the arithmetic; the
//! duplication guard test fails if a copy reappears anywhere else.
//!
//! Keep this module focused on the day-count conversion and nothing else.

/// Length of month `month` of year `year` in the proleptic Gregorian calendar.
///
/// `0` for a month outside `1..=12`, so a caller that forgets the month check
/// gets an empty day range and rejects every day rather than accepting a wrong
/// one. February is 29 days only in a leap year: divisible by 4, except every
/// 100th year, except again every 400th.
///
/// The divisibility tests use `rem_euclid` rather than `%` so the rule holds for
/// years before the common era as well, where truncating `%` would give a
/// negative remainder and answer wrongly.
///
/// This lives beside the day-count conversion because it is the same calendar,
/// and because a reader that has to place a date has to know how long the month
/// it read is. A caller must call this instead of carrying its own month table.
#[must_use]
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Whether `year` has a 29th of February in the proleptic Gregorian calendar.
#[must_use]
pub fn is_leap_year(year: i64) -> bool {
    (year.rem_euclid(4) == 0 && year.rem_euclid(100) != 0) || year.rem_euclid(400) == 0
}

/// Days since 1970-01-01 → `(year, month, day)` (Howard Hinnant's
/// `civil_from_days`).///
/// The day of the year stays zero-based when the month is derived from
/// `(5 * doy + 2) / 153`; folding the day-of-month `+ 1` into a one-based day
/// of year before that step rolls every month end onto day zero of the next
/// month. The era term is floored division, so negative day counts (dates
/// before 1970) convert correctly.
///
/// Pure: a function of its argument only. It reads no clock, environment, or
/// other ambient state, which is what makes it testable deterministically.
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(0);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(0);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// `(year, month, day)` → days since 1970-01-01 (Howard Hinnant's
/// `days_from_civil`), the inverse of [`civil_from_days`].
///
/// This is the same era arithmetic read backwards, with the same Euclidean
/// divisions, so a round trip over a day count returns the triple it came
/// from. Jan and Feb shift the year down first so March starts the internal
/// year; the `+9`/`-3` branch maps the civil month onto March..Feb without a
/// modulo, and the `(153 * m + 2) / 5` term is the linear form of the
/// days-after-March-1 table.
///
/// Preconditions, as in the published algorithm: `month` is in `1..=12` and
/// `day` is in `1..=31`. Nothing here validates them, because this function
/// only converts an already-parsed triple; the reader that parses a stamp out
/// of text is the trust boundary and checks the ranges.
///
/// Pure: a function of its arguments only. It reads no clock, environment, or
/// other ambient state, which is what makes it testable deterministically.
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = u32::try_from(y - era * 400).unwrap_or(0);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + i64::from(doe) - 719_468
}

#[cfg(test)]
#[path = "civil_tests.rs"]
mod civil_tests;

#[cfg(test)]
#[path = "civil_month_tests.rs"]
mod civil_month_tests;
