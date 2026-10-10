// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the stamp helpers behind the
//! `stats` reporter: stamp parsing and the band-edge shift.
//!
//! ## Owns
//! - `parse_utc_stamp` shape and calendar rejection tests
//! - The fractional-second tail handling test
//! - The band-edge whole-second truncation test
//!
//! ## Does not own
//! - The stats report counts (see `stats_tests.rs`)
//! - Band-edge arithmetic itself (see `stats.rs`)
//!
//! ## Boundary
//!
//! Changes to stamp helper validation coverage should normally require changes
//! here. Changes to stats counting or report shape should not require changes
//! here.
//!
//! Keep this module focused on stamp helper tests.

use clio_types::ErrorCode;

#[test]
fn fractional_second_tail_is_dropped() {
    let whole = super::parse_utc_stamp("2026-03-01T12:34:56Z").unwrap();
    let tail = super::parse_utc_stamp("2026-03-01T12:34:56.987Z").unwrap();
    assert_eq!(tail, whole);
}

#[test]
fn wrong_shape_stamp_is_rejected() {
    let err = super::parse_utc_stamp("2026-03-01T12:34Z").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn month_thirteen_is_rejected() {
    let err = super::parse_utc_stamp("2026-13-01T12:34:56Z").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn february_thirtieth_is_rejected() {
    let err = super::parse_utc_stamp("2026-02-30T12:34:56Z").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn fractional_shift_truncates_to_the_whole_second() {
    // One tenth of a half-life of one tenth of a day is a fraction of a
    // second over 864 seconds in floating point; the edge keeps only the
    // whole 864 seconds, per the whole-second edge contract.
    let edge = super::band_edge("2026-03-01T00:00:00Z", 0.1, 0.1).unwrap();
    assert_eq!(edge, "2026-02-28T23:45:36Z", "{edge}");
}
