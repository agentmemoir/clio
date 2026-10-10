// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the store's stamp reader: which timestamp
//! forms it places, which it refuses, and that a date it cannot place is reported
//! as unknown rather than shifted into a month that exists.
//!
//! ## Owns
//! - The forms the reader accepts, in both the epoch and the ISO spelling
//! - The calendar validity of the date part, against the shared calendar
//! - The list of stamps the reader must refuse
//!
//! ## Does not own
//! - Which state a placed stamp puts a unit in (`distilled_freshness_tests`)
//! - The shape of the freshness answer (`distilled_freshness_shape_tests`)
//! - The calendar itself (`clio_types::civil`)
//!
//! ## Boundary
//!
//! Changes to which stamps this store can place should normally require changes
//! here. A change to the freshness states should require
//! `distilled_freshness_tests`.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_freshness::stamp_millis;

/// The instant the maintenance path stamps, in epoch millis, and the same
/// instant in the ISO form the MCP write path produces. Both are literals, like
/// every stamp in these tests, so nothing here depends on when the suite ran.
const RECONCILED_AT: &str = "1767225600000"; // 2026-01-01T00:00:00Z
const RECONCILED_AT_ISO: &str = "2026-01-01T00:00:00Z";

#[test]
fn the_stamp_reader_places_exactly_the_forms_this_store_writes() {
    let epoch_day = 19_782i64 * 86_400_000; // 2024-02-29
    let reconciled = clio_types::days_from_civil(2026, 1, 1) * 86_400_000;
    assert_eq!(stamp_millis("0"), Some(0));
    assert_eq!(stamp_millis(RECONCILED_AT), Some(reconciled));
    assert_eq!(stamp_millis(RECONCILED_AT_ISO), Some(reconciled));
    assert_eq!(
        stamp_millis("2024-02-29"),
        Some(epoch_day),
        "date-only is midnight UTC"
    );
    assert_eq!(
        stamp_millis("2024-02-29T12:34:56Z"),
        Some(epoch_day + 45_296_000),
        "a full stamp is that many millis past midnight"
    );
    assert_eq!(
        stamp_millis("2024-02-29T00:00:00.5Z"),
        Some(epoch_day + 500),
        "a fraction is kept to the millisecond"
    );
    assert_eq!(
        stamp_millis(" 2024-02-29T00:00:00Z "),
        Some(epoch_day),
        "surrounding space"
    );
    // Everything else is unknown rather than guessed.
    for unplaced in UNPLACEABLE {
        assert_eq!(stamp_millis(unplaced), None, "{unplaced:?} is not a stamp");
    }
}

/// The stamps the reader above must refuse: a date it cannot place, a time it
/// cannot place, and text that is not a stamp. An offset zone is on this list on
/// purpose — this store writes UTC only, and a stamp carrying a zone this reader
/// does not apply would be placed at the wrong instant, which is worse than
/// admitting it cannot place it.
const UNPLACEABLE: &[&str] = &[
    "",
    "   ",
    "2026-13-01T00:00:00Z",
    "2026-01-32",
    "2026-01-01T25:00:00Z",
    "2026-01-01T00:00:00+05:30",
    "yesterday",
    "2026/01/01",
    // A day the named month does not have, and a 29 February in a year without
    // one. Both are inside the 1..=31 width the reader used to check, and the
    // conversion it hands them to does not validate either, so each became a
    // real instant in the following month.
    "2025-02-29",
    "2026-02-30",
    "2026-04-31",
    "2026-06-31",
    "2026-09-31",
    "2026-11-31",
    "2025-02-29T00:00:00Z",
    "2025-02-29T12:34:56.250Z",
    // A 60th second. RFC 3339 admits one at the end of a June or December that
    // really had a leap second; nothing this store writes carries one, and the
    // reader has no table to check the position against.
    "2016-12-31T23:59:60Z",
    "2026-06-30T23:59:60Z",
    "2026-01-01T00:00:60Z",
];

/// The last day of every month, in a leap year and a common one. This is the
/// other half of the calendar check: rejecting an impossible day is only correct
/// if the last real one is still placed, and a reader that refused all 29/30/31
/// days would pass the refusal test above while breaking every month end.
#[test]
fn the_stamp_reader_still_places_the_last_real_day_of_every_month() {
    for (year, february) in [(2024, 29), (2025, 28), (2000, 29), (1900, 28)] {
        for month in 1..=12u32 {
            let last = clio_types::days_in_month(year, month);
            assert!(last >= 28, "{year}-{month:02} reported {last} days");
            let stamp = format!("{year:04}-{month:02}-{last:02}T12:00:00Z");
            let expected = clio_types::days_from_civil(year, month, last) * 86_400_000 + 43_200_000;
            assert_eq!(
                stamp_millis(&stamp),
                Some(expected),
                "{year:04}-{month:02}-{last:02} is a real date and must be placed"
            );
        }
        // The month after a February is the one a wrong February length shifts
        // into, so it is named directly rather than left to the loop above.
        assert!(
            stamp_millis(&format!("{year:04}-03-01T00:00:00Z")).is_some(),
            "the first of March exists in every year"
        );
        assert_eq!(
            february,
            clio_types::days_in_month(year, 2),
            "February {year} has {february} days"
        );
    }
}

/// A day past the end of its month is refused rather than rolled forward. The
/// reader used to accept any day up to 31 and let the conversion carry it into
/// the next month, so `2025-02-29` was reported as 2025-03-01 and a freshness
/// decision was made against an instant the writer never wrote.
#[test]
fn the_stamp_reader_refuses_a_day_past_the_end_of_its_month() {
    for (stamp, what) in [
        ("2025-02-29", "a leap day in a common year"),
        ("2026-02-30", "a thirtieth of February"),
        ("2026-01-32", "past the end of January"),
        ("2026-04-31", "a thirty-first of April"),
        ("2026-06-31", "a thirty-first of June"),
        ("2026-09-31", "a thirty-first of September"),
        ("2026-11-31", "a thirty-first of November"),
        ("1900-02-29", "a leap day in a century that is not a 400th"),
    ] {
        assert_eq!(stamp_millis(stamp), None, "{stamp} is {what}");
        assert_eq!(
            stamp_millis(&format!("{stamp}T23:59:59Z")),
            None,
            "{stamp} with a clock is still {what}"
        );
    }
}
