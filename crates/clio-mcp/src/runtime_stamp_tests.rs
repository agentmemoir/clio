// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the timestamp-resolution tests: caller
//! arguments win over now, a now stamp carries the RFC 3339 shape with a real
//! calendar date and a real clock time, and the mcp stamp path names the real
//! day on every month end.
//!
//! ## Owns
//! - The `resolve_timestamp` precedence assertion
//! - The stamp shape-and-validity check that fails on an impossible date or time
//! - The month-end and leap-day assertions driven through the `utc_stamp` seam
//!
//! ## Does not own
//! - The other runtime-wiring tests (see `runtime_tests`)
//! - The calendar arithmetic itself (see `clio-types`)
//!
//! ## Boundary
//!
//! Changes to the timestamp default or its format should normally require
//! changes here. Changes to other runtime wiring should not.
//!
//! Keep this module focused on the stamp contract.

use super::*;
use crate::runtime_context::utc_stamp;
use serde_json::json;

/// Length of month `m` of year `y`, for the stamp-validity check below.
///
/// The table is deliberately a local copy rather than a shared helper: a
/// validity check that consulted the same table as the code under test would
/// prove nothing, and the conversion's own month table lives in `clio-types`.
fn days_in_month(y: i64, m: u32) -> u32 {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    }
}

/// The stamp must be `YYYY-MM-DDThh:mm:ssZ` with a real calendar date and a
/// real clock time: every fixed position carries the right separator, every
/// other position a digit, and the date and time components are in range.
/// Asserts the shape and validity only, never the day itself, so the
/// assertion holds on any machine at any time.
fn is_real_date_stamp(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    let shaped = stamp.len() == 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z';
    if !shaped {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return false;
    }
    if !digits(11..13) || !digits(14..16) || !digits(17..19) {
        return false;
    }
    let year: i64 = stamp[..4].parse().unwrap_or(i64::MIN);
    let month: u32 = stamp[5..7].parse().unwrap_or(0);
    let day: u32 = stamp[8..10].parse().unwrap_or(0);
    let hour: u32 = stamp[11..13].parse().unwrap_or(u32::MAX);
    let minute: u32 = stamp[14..16].parse().unwrap_or(u32::MAX);
    let second: u32 = stamp[17..19].parse().unwrap_or(u32::MAX);
    (1..=days_in_month(year, month)).contains(&day) && hour < 24 && minute < 60 && second < 60
}

/// Every month end of a leap year, each as a named day count. These are the
/// days the one-based day-of-year defect clustered on, so they are the case
/// the caller-level suite has to pin: a stamp built from the defective
/// conversion answers with the wrong day for every one of them.
#[test]
fn utc_stamp_names_every_month_end_of_a_leap_year() {
    let month_ends: [(u64, i64, u32, u32); 12] = [
        (19_753, 2024, 1, 31),
        (19_782, 2024, 2, 29),
        (19_813, 2024, 3, 31),
        (19_843, 2024, 4, 30),
        (19_874, 2024, 5, 31),
        (19_904, 2024, 6, 30),
        (19_935, 2024, 7, 31),
        (19_966, 2024, 8, 31),
        (19_996, 2024, 9, 30),
        (20_027, 2024, 10, 31),
        (20_057, 2024, 11, 30),
        (20_088, 2024, 12, 31),
    ];
    for (days, year, month, day) in month_ends {
        // Noon on the day: the day count is exact and the time field is a real
        // clock time, with no dependence on any host.
        let seconds = days * 86_400 + 12 * 3_600;
        assert_eq!(
            utc_stamp(seconds),
            format!("{year:04}-{month:02}-{day:02}T12:00:00Z"),
            "month-end stamp at day count {days}"
        );
    }
}

/// The leap day and the day the defect returned in its place: 2024-02-29 must
/// not read as 2024-02-28.
#[test]
fn utc_stamp_names_the_leap_day_not_the_day_after_it() {
    let leap_day = 19_782 * 86_400 + 12 * 3_600;
    assert_eq!(utc_stamp(leap_day), "2024-02-29T12:00:00Z");
    let common_february_end = 19_416 * 86_400 + 12 * 3_600;
    assert_eq!(utc_stamp(common_february_end), "2023-02-28T12:00:00Z");
}

/// The time field is part of the contract, not just the date: a stamp built at
/// a known second carries that second's clock time.
#[test]
fn utc_stamp_carries_the_time_of_the_second_it_is_given() {
    let seconds = 19_782 * 86_400 + 23 * 3_600 + 59 * 60 + 59;
    assert_eq!(utc_stamp(seconds), "2024-02-29T23:59:59Z");
    // The first second of the following day rolls the date, so the time seam
    // is a real boundary and not an off-by-one in the day count.
    assert_eq!(utc_stamp(seconds + 1), "2024-03-01T00:00:00Z");
}

/// A stamp whose time field is out of range or not numeric fails the check
/// the mcp suite uses, so the check covers the whole stamp and not only the
/// date portion.
#[test]
fn is_real_date_stamp_rejects_a_stamp_with_a_corrupt_time() {
    for stamp in [
        "2026-09-28T24:00:00Z",
        "2026-09-28T12:60:00Z",
        "2026-09-28T12:00:60Z",
        "2026-09-28Txx:00:00Z",
        "2026-09-28T12-00-00Z",
    ] {
        assert!(!is_real_date_stamp(stamp), "accepted {stamp}");
    }
    assert!(is_real_date_stamp("2026-09-28T12:00:00Z"));
}

/// Caller `timestamp` arguments win over now; the now default ends with `Z`
/// and names a real calendar date.
#[test]
fn timestamp_args_win_over_now() {
    let s = super::runtime_tests::state();
    assert_eq!(
        s.resolve_timestamp(&json!({"timestamp": "2024-01-01T00:00:00Z"}))
            .expect("a canonical stamp is placeable"),
        "2024-01-01T00:00:00Z"
    );
    let now = s.resolve_timestamp(&json!({})).expect("now is placeable");
    assert!(now.ends_with('Z'), "now stamp: {now}");
    // The suffix check alone was green while the stamp named a day that does
    // not exist, so the date portion must also be a real calendar date. The
    // assertion is on shape and validity, never on which day it is.
    assert!(is_real_date_stamp(&now), "now stamp: {now}");
    assert!(
        is_real_date_stamp(&now_utc()),
        "now_utc stamp: {}",
        now_utc()
    );
}
