// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the writer-stamp forms and order the
//! freshness guard relies on: the instant-aware order between two encodings,
//! and the canonicaliser that puts every store legend on one second.
//!
//! ## Owns
//! - Behavior coverage for `writer_stamp_order` and `canonical_writer_stamp`
//!   at the pure-function boundary
//!
//! ## Does not own
//! - The age and future bounds, or `ApplyOutcome::Stale`
//!   (`apply_freshness_tests`)
//! - Stamp parsing (`clock_tests` covers the reader)
//!
//! ## Boundary
//!
//! These tests break when a stamp form is placed differently, the order changes,
//! or the canonicaliser changes what it accepts; they never change either.
//!
//! Test-only. Never compiled into production builds.

use super::writer_stamp_order;

/// A stamp the strict reader refuses: it names no instant at all.
const UNREADABLE: &str = "not-a-stamp";

/// The store stamps rows in more than one encoding, so two stamps must be
/// ordered by the instant each names. Text order puts `1` before `2` and would
/// call the millis form the older one; a pair this reader cannot place keeps
/// the text order it always had.
#[test]
fn stamps_are_ordered_by_instant_and_unplaceable_pairs_by_text() {
    // 1793597865118 millis is 2026-11-02T05:37:45Z, later than STAMP.
    assert_eq!(
        writer_stamp_order("1793597865118", "2026-01-01T00:00:00Z"),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00Z", "1793597865118"),
        std::cmp::Ordering::Less
    );
    // One instant in two encodings: 2026-01-01T00:00:00Z is 1767225600000 ms.
    assert_eq!(
        writer_stamp_order("1767225600000", "2026-01-01T00:00:00Z"),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        writer_stamp_order("zzz", "aaa"),
        std::cmp::Ordering::Greater,
        "neither side places, so the pair compares as text"
    );
    assert_eq!(
        writer_stamp_order("aaa", "zzz"),
        std::cmp::Ordering::Less,
        "neither side places, so the pair compares as text"
    );
}

/// Two writes inside one second order by the instant each names, not by the
/// device id the seconds comparison leaves to decide the tie.
#[test]
fn two_writes_inside_one_second_order_by_their_fraction() {
    // 2026-01-01T00:00:00Z is 1767225600000 ms; both stamps name that second.
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00.750Z", "2026-01-01T00:00:00.250Z"),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00.250Z", "2026-01-01T00:00:00.750Z"),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        writer_stamp_order("1767225600750", "1767225600250"),
        std::cmp::Ordering::Greater,
        "epoch millis already carries the sub-second part"
    );
    // The same instant in both precisions is a tie, and stays one.
    assert_eq!(
        writer_stamp_order("1767225600500", "2026-01-01T00:00:00.500Z"),
        std::cmp::Ordering::Equal
    );
    // A whole second is that second's zero fraction, so it is below any
    // fraction inside it and above the second before.
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00Z", "2026-01-01T00:00:00.001Z"),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        writer_stamp_order("2026-01-01T00:00:00Z", "2025-12-31T23:59:59.999Z"),
        std::cmp::Ordering::Greater
    );
}

/// The canonicaliser every packaging path runs before a stamp goes on the
/// wire: one encoding at whole seconds, whichever legend the store column
/// carries. The expected values are fixed literals, so a host clock cannot
/// satisfy them.
#[test]
fn the_canonical_writer_stamp_puts_every_store_legend_on_one_second() {
    use super::canonical_writer_stamp;
    let iso = "2026-01-01T00:00:00Z";
    assert_eq!(
        canonical_writer_stamp(iso).expect("an ISO stamp places"),
        iso,
        "the canonical form is left exactly as it was"
    );
    assert_eq!(
        canonical_writer_stamp("1767225600000").expect("epoch millis place"),
        iso,
        "the store's own millis stamp becomes the same second"
    );
    assert_eq!(
        canonical_writer_stamp("2026-01-01").expect("a bare date places"),
        iso,
        "a date column becomes midnight UTC"
    );
    assert_eq!(
        canonical_writer_stamp("2026-01-01T02:00:00+02:00").expect("an offset zone places"),
        iso,
        "an offset is applied and cut to UTC whole seconds"
    );
    assert_eq!(
        canonical_writer_stamp("2026-01-01T00:00:00.750Z").expect("a fraction places"),
        iso,
        "a fraction is dropped rather than carried to the wire"
    );
    assert_eq!(
        canonical_writer_stamp("2026-01-01 00:00:00").expect("the space form places"),
        iso,
        "the store's space-separated legacy form places at the same second"
    );
}

/// A stamp that names no instant is refused loudly and names the value, so a
/// packaging path can never copy it to the wire.
#[test]
fn an_unplaceable_writer_stamp_is_refused_naming_the_value() {
    use super::canonical_writer_stamp;
    use clio_types::ErrorCode as Code;
    let err = canonical_writer_stamp(UNREADABLE).expect_err("junk is refused");
    assert_eq!(err.code, Code::InvalidArgument, "the refusal code");
    assert!(
        err.message.contains(UNREADABLE),
        "the message names the value: {}",
        err.message
    );
    assert!(
        err.message.contains("transaction_time"),
        "the message names the field: {}",
        err.message
    );
    assert!(
        canonical_writer_stamp("2026-01-01T00:00:00+99:00").is_err(),
        "an offset no zone has is refused"
    );
}
