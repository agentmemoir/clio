// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one independent reference calendar, shared by
//! every test that checks the day-count conversion or the month lengths against
//! something other than themselves.
//!
//! ## Owns
//! - `ref_is_leap` and `ref_days_in_month`, the leap rule and month table
//! - `ReferenceWalker`, a day-by-day calendar that shares no arithmetic with the
//!   code under test
//!
//! ## Does not own
//! - The conversion and the month lengths themselves (see `civil`)
//! - Any assertion; this file holds no test, because a helper with several users
//!   has no single owner to be tested by
//!
//! ## Boundary
//!
//! Changes to the reference should normally require changes here and in every
//! test that compares against it. Changes to the conversion should not require
//! changes here.
//!
//! Keep this module focused on being an answer that does not come from the
//! implementation.

/// Whether `y` is a leap year in the proleptic Gregorian calendar.
///
/// Spans tested here stay in positive years, so truncating `%` is safe. This is
/// deliberately a different expression from the one in `civil`, because a
/// reference that reuses the implementation's own arithmetic agrees with it by
/// construction and proves nothing.
pub(crate) fn ref_is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Length of month `m` of year `y` (1..=12), and 0 outside the twelve months.
///
/// This table is intentionally a second, independent copy of the one in
/// `clio-mcp`'s stamp tests. A validity assertion that read the same table the
/// code under test reads would agree with it by construction, so the two tables
/// must not be shared for the checks to mean anything.
pub(crate) fn ref_days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if ref_is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// An independent reference calendar: it walks day by day from the Unix epoch
/// against a month-length table and shares no arithmetic with
/// `civil_from_days`. Agreement between the two is therefore evidence, not a
/// self-comparison.
pub(crate) struct ReferenceWalker {
    y: i64,
    m: u32,
    d: u32,
}

impl ReferenceWalker {
    /// A walker positioned at the Unix epoch.
    pub(crate) const fn new_at_epoch() -> Self {
        Self {
            y: 1970,
            m: 1,
            d: 1,
        }
    }

    /// The date the walker is on.
    pub(crate) const fn value(&self) -> (i64, u32, u32) {
        (self.y, self.m, self.d)
    }

    /// Advance one calendar day.
    pub(crate) fn next(&mut self) {
        if self.d < ref_days_in_month(self.y, self.m) {
            self.d += 1;
        } else if self.m < 12 {
            self.m += 1;
            self.d = 1;
        } else {
            self.y += 1;
            self.m = 1;
            self.d = 1;
        }
    }

    /// Step back one calendar day.
    pub(crate) fn prev(&mut self) {
        if self.d > 1 {
            self.d -= 1;
        } else if self.m > 1 {
            self.m -= 1;
            self.d = ref_days_in_month(self.y, self.m);
        } else {
            self.y -= 1;
            self.m = 12;
            self.d = ref_days_in_month(self.y, 12);
        }
    }
}
