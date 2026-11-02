// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync crate's clock seams: the
//! strict reader that turns a written UTC stamp back into epoch seconds, and
//! the single local clock read.
//!
//! ## Owns
//! - Behavior coverage for `clock`
//!
//! ## Does not own
//! - The calendar arithmetic under the reader (`clio-types` covers it)
//! - The stamp format's agreement with the store's writer
//!
//! ## Boundary
//!
//! Tests here break when the accepted stamp shape or the clock read changes;
//! they never change it.
//!
//! Test-only. Never compiled into production builds.

use super::*;

/// The epoch itself is the one value with no room for arithmetic error.
#[test]
fn parse_returns_zero_for_the_epoch() {
    assert_eq!(parse_utc_epoch("1970-01-01T00:00:00Z"), Some(0));
}

/// A known modern stamp, so a wrong day count of any size shows up.
#[test]
fn parse_places_a_known_modern_stamp() {
    assert_eq!(parse_utc_epoch("2026-01-01T00:00:00Z"), Some(1_767_225_600));
    assert_eq!(parse_utc_epoch("2026-11-02T12:34:56Z"), Some(1_793_622_896));
}

/// The time of day is added, not dropped, and the last second of a day stays
/// inside that day.
#[test]
fn parse_adds_the_time_of_day() {
    assert_eq!(parse_utc_epoch("1970-01-01T00:00:01Z"), Some(1));
    assert_eq!(parse_utc_epoch("1970-01-01T01:00:00Z"), Some(3_600));
    assert_eq!(parse_utc_epoch("1970-01-01T23:59:59Z"), Some(86_399));
}

/// A leap day is placed; the same day in a common year is refused rather than
/// rolled into March.
#[test]
fn parse_accepts_a_real_leap_day_and_refuses_a_false_one() {
    assert_eq!(parse_utc_epoch("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    assert_eq!(parse_utc_epoch("2023-02-29T00:00:00Z"), None);
    assert_eq!(parse_utc_epoch("2023-02-30T00:00:00Z"), None);
}

/// Dates before the epoch are negative, never clamped or refused.
#[test]
fn parse_places_a_stamp_before_the_epoch() {
    assert_eq!(parse_utc_epoch("1969-12-31T23:59:59Z"), Some(-1));
    assert_eq!(parse_utc_epoch("1969-01-01T00:00:00Z"), Some(-31_536_000));
}

/// Every shape the writer never writes is `None`: empty, words, a missing `Z`,
/// trailing text, a fraction, an offset zone, and a field out of range.
#[test]
fn parse_refuses_malformed_stamps() {
    for stamp in [
        "",
        "nope",
        "2026-01-01T00:00:00",
        "2026-01-01T00:00:00Z ",
        "2026-01-01T00:00:00Zx",
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:00:00+00:00",
        "2026-13-01T00:00:00Z",
        "2026-00-01T00:00:00Z",
        "2026-01-00T00:00:00Z",
        "2026-01-01T24:00:00Z",
        "2026-01-01T00:60:00Z",
        "2026-01-01T00:00:60Z",
        "2026-01-01 00:00:00Z",
        "20x6-01-01T00:00:00Z",
        "2026-1-01T00:00:00Z",
    ] {
        assert_eq!(parse_utc_epoch(stamp), None, "{stamp:?} is not a stamp");
    }
}

/// The writer's text and the reader agree on both sides of the epoch, which is
/// the contract a freshness comparison depends on.
#[test]
fn writer_text_round_trips_through_the_reader() {
    assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(format_utc(-1), "1969-12-31T23:59:59Z");
    for epoch in [
        -86_400,
        -1,
        0,
        1,
        1_709_164_800,
        1_767_225_600,
        1_793_622_896,
    ] {
        let text = format_utc(epoch);
        assert_eq!(parse_utc_epoch(&text), Some(epoch), "{text}");
    }
}

/// The clock read stays in a plausible epoch range, and the reader agrees with
/// the writer within the one second the two reads may straddle.
#[test]
fn now_epoch_is_plausible_and_agrees_with_now_utc() {
    let read = now_epoch();
    assert!(read > 1_700_000_000, "later than 2023: {read}");
    assert!(read < 4_000_000_000, "earlier than 2096: {read}");
    let written = parse_utc_epoch(&now_utc()).expect("the writer's stamp parses");
    assert!(
        (written - read).abs() <= 1,
        "writer {written} and clock {read} differ by more than one second"
    );
}
