// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of relative temporal-expression
//! resolution: supported phrases, offset-aware boundaries, the fail-closed
//! unsupported/ambiguous rules, and anchor validation.
//!
//! ## Owns
//! - Parser test cases and their expected windows
//! - The all-phrase/offset battery, calendar-boundary cases, and the
//!   resolution determinism (purity and anchor-shift) property tests
//!
//! ## Does not own
//! - Production resolution logic (`temporal`) or arm integration
//!   (`temporal_arm_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::temporal::{MAX_TEMPORAL_QUERY_BYTES, TemporalWindow, resolve_window};

/// The one window a query resolves to, panicking when it does not resolve.
fn window(query: &str, anchor: &str) -> TemporalWindow {
    resolve_window(query, anchor)
        .expect("anchor parses")
        .unwrap_or_else(|| panic!("query `{query}` must resolve one window"))
}

/// Assert a query resolves to no window.
fn no_window(query: &str, anchor: &str) {
    assert!(
        resolve_window(query, anchor)
            .expect("anchor parses")
            .is_none(),
        "query `{query}` must not resolve a window"
    );
}

fn bounds(query: &str, anchor: &str) -> (String, String) {
    let window = window(query, anchor);
    (window.start, window.end)
}

#[test]
fn today_uses_the_anchor_offset_for_day_boundaries() {
    assert_eq!(
        bounds("what did I do today", "2026-09-26T10:00:00-07:00"),
        (
            "2026-09-26T07:00:00Z".to_owned(),
            "2026-09-27T07:00:00Z".to_owned()
        )
    );
}

#[test]
fn yesterday_ends_at_the_anchor_days_start() {
    assert_eq!(
        bounds("yesterday's deploy", "2026-09-26T00:30:00+02:00"),
        (
            "2026-09-24T22:00:00Z".to_owned(),
            "2026-09-25T22:00:00Z".to_owned()
        )
    );
}

#[test]
fn this_week_starts_on_monday() {
    // 2026-09-26 is a Saturday; its ISO week starts Monday 2026-09-21.
    assert_eq!(
        bounds("this week", "2026-09-26T12:00:00Z"),
        (
            "2026-09-21T00:00:00Z".to_owned(),
            "2026-09-28T00:00:00Z".to_owned()
        )
    );
}

#[test]
fn last_week_is_the_previous_iso_week_even_on_a_monday() {
    assert_eq!(
        bounds("last week", "2026-09-21T09:00:00Z"),
        (
            "2026-09-14T00:00:00Z".to_owned(),
            "2026-09-21T00:00:00Z".to_owned()
        )
    );
}

#[test]
fn month_windows_cross_the_year_boundary() {
    assert_eq!(
        bounds("this month", "2026-01-15T00:00:00Z"),
        (
            "2026-01-01T00:00:00Z".to_owned(),
            "2026-02-01T00:00:00Z".to_owned()
        )
    );
    assert_eq!(
        bounds("last month", "2026-01-15T00:00:00Z"),
        (
            "2025-12-01T00:00:00Z".to_owned(),
            "2026-01-01T00:00:00Z".to_owned()
        )
    );
}

#[test]
fn year_windows_are_calendar_years() {
    assert_eq!(
        bounds("this year", "2026-06-01T00:00:00Z"),
        (
            "2026-01-01T00:00:00Z".to_owned(),
            "2027-01-01T00:00:00Z".to_owned()
        )
    );
    assert_eq!(
        bounds("last year", "2026-06-01T00:00:00Z"),
        (
            "2025-01-01T00:00:00Z".to_owned(),
            "2026-01-01T00:00:00Z".to_owned()
        )
    );
}

#[test]
fn an_offset_anchor_shifts_the_utc_bounds() {
    // Midnight in +05:30 is 18:30 UTC on the previous day.
    assert_eq!(
        bounds("today", "2026-09-26T00:00:00+05:30"),
        (
            "2026-09-25T18:30:00Z".to_owned(),
            "2026-09-26T18:30:00Z".to_owned()
        )
    );
}

#[test]
fn unsupported_phrases_yield_no_window() {
    let anchor = "2026-09-26T12:00:00Z";
    for query in [
        "last quarter",
        "recently",
        "in three days",
        "3 days ago",
        "last weekends",
        "week",
        "month",
        "the week before",
        "next week",
        "tomorrow",
    ] {
        no_window(query, anchor);
    }
}

#[test]
fn two_distinct_expressions_fail_closed_as_ambiguous() {
    no_window("today or last week", "2026-09-26T12:00:00Z");
    no_window(
        "what changed this week and last month",
        "2026-09-26T12:00:00Z",
    );
}

#[test]
fn a_repeated_phrase_is_still_one_window() {
    let anchor = "2026-09-26T12:00:00Z";
    let once = window("last week", anchor);
    let twice = window("last week, as I said last week", anchor);
    assert_eq!(once, twice);
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(
        window("Last Week", "2026-09-26T12:00:00Z"),
        window("last week", "2026-09-26T12:00:00Z")
    );
}

#[test]
fn whole_tokens_only() {
    no_window("todays plan", "2026-09-26T12:00:00Z");
    no_window("monthly report", "2026-09-26T12:00:00Z");
    // The possessive splits on the apostrophe and still reads as yesterday.
    assert_eq!(
        window("yesterday's meeting", "2026-09-26T12:00:00Z"),
        window("yesterday", "2026-09-26T12:00:00Z")
    );
}

#[test]
fn a_query_without_an_expression_yields_no_window() {
    no_window("postgres connection pool", "2026-09-26T12:00:00Z");
}

#[test]
fn a_malformed_anchor_is_rejected() {
    for anchor in [
        "2026-09-26",
        "2026-09-26T10:00:00",
        "26/09/2026",
        "not-a-time",
        "",
    ] {
        let error = resolve_window("last week", anchor).expect_err("anchor must be rejected");
        assert_eq!(
            error.code,
            clio_types::ErrorCode::InvalidArgument,
            "{anchor}"
        );
    }
}

#[test]
fn an_oversized_query_yields_no_window() {
    let anchor = "2026-09-26T12:00:00Z";
    let mut query = String::from("last week ");
    query.push_str(&"x".repeat(MAX_TEMPORAL_QUERY_BYTES));
    no_window(&query, anchor);
    // The same phrase under the bound still resolves.
    assert_eq!(
        bounds("last week", anchor),
        (
            "2026-09-14T00:00:00Z".to_owned(),
            "2026-09-21T00:00:00Z".to_owned()
        )
    );
}

// Battery anchors in three offsets. Each phrase's bounds below are derived by
// hand from the `temporal` module rules: local day/week/month/year boundaries
// in the anchor's own offset, ISO Monday weeks, half-open windows, emitted as
// UTC text.
const Z_ANCHOR: &str = "2026-09-26T12:00:00Z";
// -07:00 keeps the local date 2026-09-26; local midnight is 07:00 UTC.
const NEG_ANCHOR: &str = "2026-09-26T10:00:00-07:00";
// +05:30 keeps the local date 2026-09-26; local midnight is 18:30 UTC on the
// previous UTC day.
const HALF_ANCHOR: &str = "2026-09-26T00:00:00+05:30";

#[test]
fn every_phrase_resolves_exactly_against_a_utc_anchor() {
    for (query, start, end) in [
        ("today", "2026-09-26T00:00:00Z", "2026-09-27T00:00:00Z"),
        ("yesterday", "2026-09-25T00:00:00Z", "2026-09-26T00:00:00Z"),
        ("this week", "2026-09-21T00:00:00Z", "2026-09-28T00:00:00Z"),
        ("last week", "2026-09-14T00:00:00Z", "2026-09-21T00:00:00Z"),
        ("this month", "2026-09-01T00:00:00Z", "2026-10-01T00:00:00Z"),
        ("last month", "2026-08-01T00:00:00Z", "2026-09-01T00:00:00Z"),
        ("this year", "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z"),
        ("last year", "2025-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    ] {
        assert_eq!(
            bounds(query, Z_ANCHOR),
            (start.to_owned(), end.to_owned()),
            "{query}"
        );
    }
}

#[test]
fn every_phrase_resolves_exactly_against_a_negative_offset_anchor() {
    for (query, start, end) in [
        ("today", "2026-09-26T07:00:00Z", "2026-09-27T07:00:00Z"),
        ("yesterday", "2026-09-25T07:00:00Z", "2026-09-26T07:00:00Z"),
        ("this week", "2026-09-21T07:00:00Z", "2026-09-28T07:00:00Z"),
        ("last week", "2026-09-14T07:00:00Z", "2026-09-21T07:00:00Z"),
        ("this month", "2026-09-01T07:00:00Z", "2026-10-01T07:00:00Z"),
        ("last month", "2026-08-01T07:00:00Z", "2026-09-01T07:00:00Z"),
        ("this year", "2026-01-01T07:00:00Z", "2027-01-01T07:00:00Z"),
        ("last year", "2025-01-01T07:00:00Z", "2026-01-01T07:00:00Z"),
    ] {
        assert_eq!(
            bounds(query, NEG_ANCHOR),
            (start.to_owned(), end.to_owned()),
            "{query}"
        );
    }
}

#[test]
fn every_phrase_resolves_exactly_against_a_half_hour_offset_anchor() {
    // +05:30: local midnight of date D is 18:30 UTC on D−1, so every
    // local-calendar bound lands on the previous UTC day.
    for (query, start, end) in [
        ("today", "2026-09-25T18:30:00Z", "2026-09-26T18:30:00Z"),
        ("yesterday", "2026-09-24T18:30:00Z", "2026-09-25T18:30:00Z"),
        ("this week", "2026-09-20T18:30:00Z", "2026-09-27T18:30:00Z"),
        ("last week", "2026-09-13T18:30:00Z", "2026-09-20T18:30:00Z"),
        ("this month", "2026-08-31T18:30:00Z", "2026-09-30T18:30:00Z"),
        ("last month", "2026-07-31T18:30:00Z", "2026-08-31T18:30:00Z"),
        ("this year", "2025-12-31T18:30:00Z", "2026-12-31T18:30:00Z"),
        ("last year", "2024-12-31T18:30:00Z", "2025-12-31T18:30:00Z"),
    ] {
        assert_eq!(
            bounds(query, HALF_ANCHOR),
            (start.to_owned(), end.to_owned()),
            "{query}"
        );
    }
}

#[test]
fn calendar_boundaries_resolve_to_the_documented_windows() {
    for (query, anchor, start, end) in [
        // Jan 1 anchors roll the month and year arithmetic back a year.
        (
            "this month",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
        ),
        (
            "last month",
            "2026-01-01T00:00:00Z",
            "2025-12-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
        (
            "last year",
            "2026-12-31T23:00:00Z",
            "2025-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
        // The Dec 31 / Jan 1 edge on the year phrases.
        (
            "this year",
            "2026-12-31T23:00:00Z",
            "2026-01-01T00:00:00Z",
            "2027-01-01T00:00:00Z",
        ),
        (
            "this year",
            "2027-01-01T01:00:00Z",
            "2027-01-01T00:00:00Z",
            "2028-01-01T00:00:00Z",
        ),
        (
            "last year",
            "2027-01-01T01:00:00Z",
            "2026-01-01T00:00:00Z",
            "2027-01-01T00:00:00Z",
        ),
        // 2026-12-31 is a Thursday, so its ISO week spans the year boundary.
        (
            "this week",
            "2026-12-31T12:00:00Z",
            "2026-12-28T00:00:00Z",
            "2027-01-04T00:00:00Z",
        ),
        (
            "last week",
            "2026-12-31T12:00:00Z",
            "2026-12-21T00:00:00Z",
            "2026-12-28T00:00:00Z",
        ),
        // 2024-02-29 exists only because 2024 is a leap year.
        (
            "this year",
            "2024-02-29T12:00:00Z",
            "2024-01-01T00:00:00Z",
            "2025-01-01T00:00:00Z",
        ),
        (
            "this month",
            "2024-02-29T12:00:00Z",
            "2024-02-01T00:00:00Z",
            "2024-03-01T00:00:00Z",
        ),
        // A month-end anchor must not shorten `last month`'s window.
        (
            "last month",
            "2026-01-31T12:00:00Z",
            "2025-12-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
    ] {
        assert_eq!(
            bounds(query, anchor),
            (start.to_owned(), end.to_owned()),
            "{query} @ {anchor}"
        );
    }
}

/// The resolution functions take no clock input: `resolve_window`'s only
/// inputs are `query` and `anchor`, so determinism holds by construction.
/// These tests pin the observable part of that property.
#[test]
fn resolve_window_is_pure_in_query_and_anchor() {
    for query in [
        "today",
        "yesterday",
        "this week",
        "last week",
        "this month",
        "last month",
        "this year",
        "last year",
    ] {
        for anchor in [Z_ANCHOR, NEG_ANCHOR, HALF_ANCHOR] {
            let first = window(query, anchor);
            let second = window(query, anchor);
            assert_eq!(first, second, "{query} @ {anchor}");
        }
    }
}

/// Add `days` to a UTC bound string of the window format.
fn plus_days(bound: &str, days: i64) -> String {
    let time =
        chrono::NaiveDateTime::parse_from_str(bound, "%Y-%m-%dT%H:%M:%SZ").expect("a window bound");
    (time + chrono::Duration::days(days))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

#[test]
fn a_one_day_anchor_shift_moves_day_bounds_by_exactly_one_day() {
    for query in ["today", "yesterday"] {
        let base = window(query, "2026-09-26T12:00:00Z");
        let shifted = window(query, "2026-09-27T12:00:00Z");
        assert_eq!(shifted.start, plus_days(&base.start, 1), "{query} start");
        assert_eq!(shifted.end, plus_days(&base.end, 1), "{query} end");
    }
}

#[test]
fn a_one_week_anchor_shift_moves_week_bounds_by_exactly_seven_days() {
    for query in ["this week", "last week"] {
        let base = window(query, "2026-09-26T12:00:00Z");
        let shifted = window(query, "2026-10-03T12:00:00Z");
        assert_eq!(shifted.start, plus_days(&base.start, 7), "{query} start");
        assert_eq!(shifted.end, plus_days(&base.end, 7), "{query} end");
    }
}
