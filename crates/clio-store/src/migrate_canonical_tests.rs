// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the canonical item-timestamp form: epoch
//! millis and an RFC 3339 date-time on any offset are placed on the same UTC
//! clock at second precision, an already-canonical value is left alone, a value
//! carrying a fraction is cut to the second it names, and a value this build
//! cannot place is returned untouched.
//!
//! ## Owns
//! - `canonical_ts` and `millis_to_rfc3339` on the values the app stores
//! - `canonical_write_stamp` / `canonical_optional_stamp`, the write-path
//!   refusal for a value this build cannot place
//!
//! ## Does not own
//! - The rewrite of stored rows (`migrate_ts_backfill_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{canonical_optional_stamp, canonical_ts, canonical_write_stamp, millis_to_rfc3339};

#[test]
fn millis_are_converted_to_utc_seconds_with_a_z_suffix() {
    assert_eq!(millis_to_rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(millis_to_rfc3339(1_700_000_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(millis_to_rfc3339(1_700_000_999_999), "2023-11-14T22:29:59Z");
}

#[test]
fn an_epoch_stamp_becomes_the_canonical_form() {
    assert_eq!(canonical_ts("1700000000000"), "2023-11-14T22:13:20Z");
    assert_eq!(canonical_ts("0"), "1970-01-01T00:00:00Z");
}

#[test]
fn a_canonical_value_is_unchanged() {
    assert_eq!(canonical_ts("2026-10-12T08:30:01Z"), "2026-10-12T08:30:01Z");
}

#[test]
fn an_offset_is_applied_so_the_value_keeps_its_instant() {
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01+02:00"),
        "2026-10-12T06:30:01Z",
        "east of UTC is earlier in UTC, not the same wall clock"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01-05:30"),
        "2026-10-12T14:00:01Z",
        "west of UTC is later in UTC"
    );
}

#[test]
fn an_offset_with_a_fraction_keeps_the_instant_and_loses_no_hour() {
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.500+02:00"),
        "2026-10-12T06:30:01Z",
        "the offset is applied and the fraction truncated, never the reverse"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.500Z"),
        "2026-10-12T08:30:01Z"
    );
}

#[test]
fn a_fraction_is_cut_because_the_column_is_compared_at_one_precision() {
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.999999Z"),
        "2026-10-12T08:30:01Z"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.5Z"),
        "2026-10-12T08:30:01Z"
    );
}

#[test]
fn an_invalid_calendar_date_is_returned_unchanged() {
    assert_eq!(canonical_ts("2026-13-01T00:00:00Z"), "2026-13-01T00:00:00Z");
    assert_eq!(canonical_ts("2026-01-45T00:00:00Z"), "2026-01-45T00:00:00Z");
    assert_eq!(
        canonical_ts("2026-02-29T00:00:00Z"),
        "2026-02-29T00:00:00Z",
        "2026 is not a leap year, so the day is not a day"
    );
    assert_eq!(canonical_ts("2024-02-29T00:00:00Z"), "2024-02-29T00:00:00Z");
    assert_eq!(canonical_ts("2026-01-01T24:00:00Z"), "2026-01-01T24:00:00Z");
    assert_eq!(canonical_ts("2026-01-01T00:60:00Z"), "2026-01-01T00:60:00Z");
    assert_eq!(canonical_ts("2026-01-01T00:00:60Z"), "2026-01-01T00:00:60Z");
}

#[test]
fn a_bad_offset_is_returned_unchanged() {
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01+24:00"),
        "2026-10-12T08:30:01+24:00"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01+02:60"),
        "2026-10-12T08:30:01+02:60"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01+0200"),
        "2026-10-12T08:30:01+0200"
    );
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.Z"),
        "2026-10-12T08:30:01.Z",
        "a fraction with no digits is not a fraction"
    );
}

#[test]
fn a_value_this_build_cannot_place_is_returned_unchanged() {
    assert_eq!(canonical_ts("2026-10-12"), "2026-10-12");
    assert_eq!(canonical_ts("not a stamp"), "not a stamp");
    assert_eq!(canonical_ts(""), "");
}

#[test]
fn the_canonical_form_is_a_fixed_point() {
    for raw in [
        "1700000000000",
        "2026-10-12T08:30:01.500+02:00",
        "2026-10-12T08:30:01Z",
    ] {
        let once = canonical_ts(raw);
        assert_eq!(canonical_ts(&once), once, "second pass moved {raw}");
    }
}

#[test]
fn a_placeable_write_stamp_comes_back_canonical() {
    assert_eq!(
        canonical_write_stamp("2026-10-12T08:30:01.500+02:00").unwrap(),
        "2026-10-12T06:30:01Z"
    );
    assert_eq!(
        canonical_write_stamp("1700000000000").unwrap(),
        "2023-11-14T22:13:20Z"
    );
}

#[test]
fn an_unplaceable_write_stamp_is_refused_with_invalid_argument() {
    for raw in ["2026-02-30T00:00:00Z", "2026-10-12", "not a stamp", ""] {
        let err = canonical_write_stamp(raw).expect_err("unplaceable");
        assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument, "{raw}");
        assert!(
            err.message
                .contains("is not a timestamp this build can place"),
            "{}",
            err.message
        );
    }
}

#[test]
fn an_absent_optional_stamp_stays_absent() {
    assert_eq!(canonical_optional_stamp(None).unwrap(), None);
    assert_eq!(canonical_optional_stamp(Some("")).unwrap(), None);
}

#[test]
fn an_optional_stamp_is_canonicalised_when_present() {
    assert_eq!(
        canonical_optional_stamp(Some("2026-10-12T08:30:01.500Z")).unwrap(),
        Some("2026-10-12T08:30:01Z".to_owned())
    );
}

#[test]
fn an_unplaceable_optional_stamp_is_refused() {
    assert_eq!(
        canonical_optional_stamp(Some("2026-10-12T08:30:01.500+24:00"))
            .expect_err("unplaceable")
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
}
