// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the month lengths and the leap rule: that they
//! agree with an independent reference, that they hold before the common era, and
//! that they bound exactly the days the day-count conversion produces.
//!
//! ## Owns
//! - `days_in_month` and `is_leap_year` against a reference that is not the
//!   implementation
//! - The agreement between a month's length and the day counts around its edges
//!
//! ## Does not own
//! - The conversion itself, in both directions (`civil_tests`)
//! - The reference calendar (`civil_test_support`)
//! - Any stamp a store reads from text
//!
//! ## Boundary
//!
//! Changes to the leap rule or a month's length should normally require changes
//! here. Changes to the era arithmetic should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use super::{civil_from_days, days_from_civil, days_in_month, is_leap_year};
use crate::civil_test_support::{ref_days_in_month, ref_is_leap};

/// The month lengths agree with the independent reference table, for every
/// month of a leap year, a common year, and both kinds of century year. The
/// reference is a separate table, deliberately not the code's own match arms, so
/// agreement is evidence rather than a self-comparison.
#[test]
fn days_in_month_agrees_with_an_independent_reference() {
    for year in [1900, 1970, 1999, 2000, 2024, 2025, 2026, 2100, 2400] {
        for month in 0..=13u32 {
            assert_eq!(
                days_in_month(year, month),
                ref_days_in_month(year, month),
                "{year}-{month:02} disagrees with the reference"
            );
        }
    }
}

/// Every month has the length the calendar gives it, named per month so a
/// failure says which month moved rather than only which year.
#[test]
fn days_in_month_returns_the_named_month_lengths() {
    for (month, length) in [
        (1, 31),
        (2, 28),
        (3, 31),
        (4, 30),
        (5, 31),
        (6, 30),
        (7, 31),
        (8, 31),
        (9, 30),
        (10, 31),
        (11, 30),
        (12, 31),
    ] {
        assert_eq!(days_in_month(2025, month), length, "month {month}");
    }
    assert_eq!(days_in_month(2024, 2), 29, "February in a leap year");
    assert_eq!(days_in_month(2000, 2), 29, "February in a 400th year");
    assert_eq!(
        days_in_month(1900, 2),
        28,
        "February in a non-400th century"
    );
    assert_eq!(
        days_in_month(2100, 2),
        28,
        "February in a non-400th century"
    );
}

/// A month outside the calendar reports zero days rather than a plausible
/// number, so a caller that skips its own month check rejects every day instead
/// of accepting a wrong one.
#[test]
fn days_in_month_is_zero_outside_the_twelve_months() {
    for month in [0, 13, 14, 99, u32::MAX] {
        assert_eq!(days_in_month(2024, month), 0, "month {month}");
    }
}

/// The leap rule holds before the common era, where a truncating remainder would
/// answer wrongly. Year 0 and year -4 are leap; year -1 and year -100 are not.
#[test]
fn is_leap_year_holds_for_years_before_the_common_era() {
    for year in [-400, -100, -101, -4, -1, 0, 4, 100, 400] {
        assert_eq!(
            is_leap_year(year),
            ref_is_leap(year),
            "year {year} disagrees with the reference"
        );
    }
    assert!(is_leap_year(0), "year 0 is divisible by 400");
    assert!(is_leap_year(-4), "year -4 is divisible by 4");
    assert!(!is_leap_year(-1), "year -1 is divisible by none of them");
    assert!(
        !is_leap_year(-100),
        "year -100 is divisible by 100 but not 400"
    );
}

/// Every day a month claims exists is one day after the day before it, across a
/// full leap year including the leap February, and the day after each month's
/// last is the next month's first. A month length that disagreed with the
/// conversion would show up here as a jump or a stall.
#[test]
fn the_month_lengths_agree_with_the_day_count_they_bound() {
    for month in 1..=12u32 {
        let last = days_in_month(2024, month);
        assert!((28..=31).contains(&last), "month {month} has {last} days");
        // This month's first day, so the first comparison is its second day
        // against its own first. Resetting per month is what makes a month
        // length that disagrees with the conversion visible: the cursor would
        // otherwise still be sitting in the previous month.
        let mut previous = days_from_civil(2024, month, 1);
        for day in 2..=last {
            let current = days_from_civil(2024, month, day);
            assert_eq!(
                current,
                previous + 1,
                "2024-{month:02}-{day:02} is not the day after the one before"
            );
            previous = current;
        }
        let next = days_from_civil(2024, month, last) + 1;
        let expected = if month == 12 {
            (2025, 1, 1)
        } else {
            (2024, month + 1, 1)
        };
        assert_eq!(
            civil_from_days(next),
            expected,
            "the day after 2024-{month:02}-{last:02}"
        );
    }
}
