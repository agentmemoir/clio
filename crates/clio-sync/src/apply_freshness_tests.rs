// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the envelope freshness guard: the
//! age bound in the past, the separate future allowance, the disabled bound,
//! the unreadable stamp, and the guard's place in the apply order.
//!
//! ## Owns
//! - Behavior coverage for `apply_freshness` and `ApplyOutcome::Stale`
//!
//! ## Does not own
//! - Stamp parsing (`clock_tests` covers the reader)
//! - The apply matrix and its other outcomes (`apply_tests`)
//! - Cursor persistence on a stale page (`client_freshness_tests`)
//!
//! ## Boundary
//!
//! These tests break when the freshness rule or its apply-order placement
//! changes; they never change either.
//!
//! Test-only. Never compiled into production builds.

use super::{is_stale, pinned, writer_stamp_epoch, writer_stamp_order};
use crate::ENVELOPE_SKEW_DEFAULT_SECS;
use crate::apply::apply_freshness::ENVELOPE_FUTURE_ALLOWANCE_SECS;
use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation};
use crate::apply_fixtures::{fixture, upsert};
use crate::clock::parse_utc_epoch;
use crate::protocol::SyncMutation;
use clio_store::{SqliteStore, Store, SyncStore};
use std::sync::Arc;

/// The fixed writer stamp every case in the table starts from.
const STAMP: &str = "2026-01-01T00:00:00Z";

/// The positive age bound the table runs at: the value an always-online fleet
/// would set, so the rule is exercised at a real setting, not only at `0`.
const WINDOW: u64 = 300;

/// A stamp the strict reader refuses: it names no instant at all.
const UNREADABLE: &str = "not-a-stamp";

/// Apply `m` against `store` with the clock pinned to `now` and the window set
/// to `skew`.
fn apply_at(store: &Arc<SqliteStore>, m: &SyncMutation, now: i64, skew: u64) -> ApplyOutcome {
    let engine = ApplyEngine {
        store: store.as_ref(),
        sync: store.as_ref(),
        device_id: "node-1".into(),
        kms: None,
        clock: pinned(now),
        max_envelope_skew_secs: skew,
    };
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    apply_mutation(&engine, m, &envelope).expect("apply runs")
}

/// One table case: the writer stamp, the clock, the age bound, and the outcome
/// the guard must produce.
struct Case {
    label: &'static str,
    stamp: &'static str,
    now: i64,
    skew: u64,
    expected: ApplyOutcome,
}

/// Every freshness outcome, from one table, so a rule that flips direction is
/// visible in one place.
#[test]
fn the_bounds_decide_every_stamp_by_direction_and_distance() {
    let written = parse_utc_epoch(STAMP).expect("the fixture stamp parses");
    let ahead = i64::try_from(ENVELOPE_FUTURE_ALLOWANCE_SECS).expect("the allowance fits an i64");
    let cases = [
        Case {
            label: "a stamp ten seconds old applies",
            stamp: STAMP,
            now: written + 10,
            skew: WINDOW,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a stamp older than the age bound is stale",
            stamp: STAMP,
            now: written + 301,
            skew: WINDOW,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a stamp exactly at the age bound applies",
            stamp: STAMP,
            now: written + 300,
            skew: WINDOW,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a stamp exactly at the future allowance applies",
            stamp: STAMP,
            now: written - ahead,
            skew: WINDOW,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a stamp beyond the future allowance is stale",
            stamp: STAMP,
            now: written - ahead - 1,
            skew: WINDOW,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a zero age bound accepts an old stamp",
            stamp: STAMP,
            now: written + 10_000_000,
            skew: 0,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a zero age bound accepts a far-future stamp",
            stamp: STAMP,
            now: written - 10_000_000,
            skew: 0,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "an unplaceable stamp is refused",
            stamp: UNREADABLE,
            now: written,
            skew: WINDOW,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a zero age bound also disables the unplaceable-stamp refusal",
            stamp: UNREADABLE,
            now: written,
            skew: 0,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "an extreme clock reading does not panic",
            stamp: STAMP,
            now: i64::MAX,
            skew: WINDOW,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a clock before the epoch does not panic",
            stamp: STAMP,
            now: i64::MIN,
            skew: WINDOW,
            expected: ApplyOutcome::Stale,
        },
    ];
    for case in cases {
        let (store, _kms) = fixture();
        let mut m = upsert("m-fresh", "b-fresh", STAMP, "dev-a", "gist");
        m.transaction_time = case.stamp.into();
        let outcome = apply_at(&store, &m, case.now, case.skew);
        assert_eq!(outcome, case.expected, "{}", case.label);
        let row = store.get_memory_item("b-fresh", "i-1").expect("read item");
        let durable = matches!(case.expected, ApplyOutcome::Applied);
        assert_eq!(
            row.is_some(),
            durable,
            "{}: durable state must follow the outcome",
            case.label
        );
    }
}

/// The shipped default is no age bound, so an ordinary backlog applies. A
/// positive default would drop every row an offline node stored, so this pins
/// the value as well as the behaviour it buys.
#[test]
fn the_shipped_default_accepts_a_ten_minute_old_row() {
    assert_eq!(
        ENVELOPE_SKEW_DEFAULT_SECS, 0,
        "the shipped age bound must stay off: a backlog older than any window is legitimate"
    );
    let (store, _kms) = fixture();
    let m = upsert("m-backlog", "b-backlog", STAMP, "dev-a", "gist");
    let written = parse_utc_epoch(STAMP).expect("the fixture stamp parses");
    assert_eq!(
        apply_at(&store, &m, written + 600, ENVELOPE_SKEW_DEFAULT_SECS),
        ApplyOutcome::Applied,
        "a ten-minute-old row must apply under the shipped default"
    );
}

/// A stale envelope journals nothing, so the next delivery of the same id is
/// stale again instead of reporting a duplicate of an apply that never ran.
#[test]
fn a_stale_envelope_writes_no_journal_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-stale", "b-stale", STAMP, "dev-a", "gist");
    let now = parse_utc_epoch(STAMP).expect("stamp parses") + 3_600;
    assert_eq!(
        apply_at(&store, &m, now, WINDOW),
        ApplyOutcome::Stale,
        "an hour-old row is stale at a 300 second bound"
    );
    assert!(
        store.journal_get("b-stale", "m-stale").unwrap().is_none(),
        "a stale envelope must not be journaled"
    );
    assert_eq!(
        apply_at(&store, &m, now, WINDOW),
        ApplyOutcome::Stale,
        "no journal row means the second delivery is stale, not a duplicate"
    );
}

/// The store stamps item rows two ways, and an item's stamp travels unchanged
/// as an envelope's `transaction_time`. A documented legacy form must be placed
/// by its time, so an in-boundary row is never refused for its shape.
#[test]
fn a_documented_legacy_stamp_is_placed_instead_of_refused() {
    let secs = parse_utc_epoch(STAMP).expect("the fixture stamp parses");
    let millis = format!("{secs}000");
    assert_eq!(writer_stamp_epoch(STAMP), Some(secs));
    assert_eq!(writer_stamp_epoch("2026-01-01T00:00:00.750Z"), Some(secs));
    assert_eq!(writer_stamp_epoch("2026-01-01 00:00:00"), Some(secs));
    assert_eq!(writer_stamp_epoch("2026-01-01T00:00:00"), Some(secs));
    assert_eq!(writer_stamp_epoch("2026-01-01"), Some(secs));
    assert_eq!(writer_stamp_epoch(&millis), Some(secs));
    assert_eq!(
        writer_stamp_epoch("2026-01-01T00:00:00+02:00"),
        None,
        "an offset zone names no instant this reader places"
    );
    assert_eq!(writer_stamp_epoch("not-a-stamp"), None);
    // The placed millis form is bounded by its time, not by its shape.
    let (store, _kms) = fixture();
    let engine = ApplyEngine {
        store: store.as_ref(),
        sync: store.as_ref(),
        device_id: "node-1".into(),
        kms: None,
        clock: pinned(secs + 10),
        max_envelope_skew_secs: WINDOW,
    };
    assert!(!is_stale(&engine, &millis), "ten seconds old is fresh");
}

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

/// The id pre-check runs first: a journaled id answers `Duplicate` even when
/// the same envelope would be stale against the clock.
#[test]
fn a_journaled_id_still_answers_duplicate_before_the_age_bound() {
    let (store, _kms) = fixture();
    let m = upsert("m-dup", "b-dup", STAMP, "dev-a", "gist");
    let written = parse_utc_epoch(STAMP).expect("stamp parses");
    assert_eq!(apply_at(&store, &m, written, WINDOW), ApplyOutcome::Applied);
    assert!(
        store.journal_get("b-dup", "m-dup").unwrap().is_some(),
        "the applied envelope is journaled"
    );
    assert_eq!(
        apply_at(&store, &m, written + 86_400, WINDOW),
        ApplyOutcome::Duplicate,
        "the journal wins over the age bound"
    );
}
