// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the canonical item-timestamp form: epoch
//! millis are converted, an already-canonical value is left alone, and a
//! value carrying a fraction is cut to the second it names.
//!
//! ## Owns
//! - `canonical_ts` and `millis_to_rfc3339` on the values the app stores
//!
//! ## Does not own
//! - The rewrite of stored rows (`migrate_ts_backfill_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{canonical_ts, millis_to_rfc3339};

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
fn a_fraction_is_cut_because_the_column_is_compared_at_one_precision() {
    assert_eq!(
        canonical_ts("2026-10-12T08:30:01.500Z"),
        "2026-10-12T08:30:01Z"
    );
}

#[test]
fn a_value_this_build_cannot_place_is_returned_unchanged() {
    assert_eq!(canonical_ts("2026-10-12"), "2026-10-12");
    assert_eq!(canonical_ts("not a stamp"), "not a stamp");
    assert_eq!(canonical_ts(""), "");
}
