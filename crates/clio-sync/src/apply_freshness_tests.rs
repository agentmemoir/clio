// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the envelope freshness guard: the
//! symmetric window around the injected clock, the disabled window, the
//! unreadable stamp, and the guard's place in the apply order.
//!
//! ## Owns
//! - Behavior coverage for `apply_freshness` and `ApplyOutcome::Stale`
//!
//! ## Does not own
//! - Stamp parsing (`clock_tests` covers the reader)
//! - The apply matrix and its other outcomes (`apply_tests`)
//! - Cursor persistence on a stale page (`client_tests`)
//!
//! ## Boundary
//!
//! These tests break when the freshness rule or its apply-order placement
//! changes; they never change either.
//!
//! Test-only. Never compiled into production builds.

use super::pinned;
use crate::ENVELOPE_SKEW_DEFAULT_SECS;
use crate::apply::apply_tests::{fixture, upsert};
use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation};
use crate::clock::parse_utc_epoch;
use crate::protocol::SyncMutation;
use clio_store::{SqliteStore, Store, SyncStore};
use std::sync::Arc;

/// The fixed writer stamp every case in the table starts from.
const STAMP: &str = "2026-01-01T00:00:00Z";

/// A stamp the strict reader refuses: it names no instant at all.
const UNREADABLE: &str = "2026-01-01 00:00:00";

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

/// One table case: the writer stamp, the clock, the window, and the outcome the
/// guard must produce.
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
fn the_window_decides_every_stamp_by_distance_only() {
    let written = parse_utc_epoch(STAMP).expect("the fixture stamp parses");
    let cases = [
        Case {
            label: "a stamp inside the window applies",
            stamp: STAMP,
            now: written + 10,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a stamp older than the window is stale",
            stamp: STAMP,
            now: written - 301,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a future stamp beyond the window is stale",
            stamp: STAMP,
            now: written + 301,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a stamp exactly at the past bound applies",
            stamp: STAMP,
            now: written - 300,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a stamp exactly at the future bound applies",
            stamp: STAMP,
            now: written + 300,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "a zero window disables the check",
            stamp: STAMP,
            now: written - 10_000_000,
            skew: 0,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "an unreadable stamp is stale",
            stamp: UNREADABLE,
            now: written,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a zero window also disables the unreadable-stamp refusal",
            stamp: UNREADABLE,
            now: written,
            skew: 0,
            expected: ApplyOutcome::Applied,
        },
        Case {
            label: "an extreme clock reading does not panic",
            stamp: STAMP,
            now: i64::MAX,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
            expected: ApplyOutcome::Stale,
        },
        Case {
            label: "a clock before the epoch does not panic",
            stamp: STAMP,
            now: i64::MIN,
            skew: ENVELOPE_SKEW_DEFAULT_SECS,
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

/// A stale envelope journals nothing, so the next delivery of the same id is
/// stale again instead of reporting a duplicate of an apply that never ran.
#[test]
fn a_stale_envelope_writes_no_journal_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-stale", "b-stale", STAMP, "dev-a", "gist");
    let now = parse_utc_epoch(STAMP).expect("stamp parses") - 3_600;
    assert_eq!(
        apply_at(&store, &m, now, ENVELOPE_SKEW_DEFAULT_SECS),
        ApplyOutcome::Stale
    );
    assert!(
        store.journal_get("b-stale", "m-stale").unwrap().is_none(),
        "a stale envelope must not be journaled"
    );
    assert_eq!(
        apply_at(&store, &m, now, ENVELOPE_SKEW_DEFAULT_SECS),
        ApplyOutcome::Stale,
        "no journal row means the second delivery is stale, not a duplicate"
    );
}

/// The id pre-check runs first: a journaled id answers `Duplicate` even when
/// the same envelope would be stale against the clock.
#[test]
fn a_journaled_id_still_answers_duplicate_before_the_window() {
    let (store, _kms) = fixture();
    let m = upsert("m-dup", "b-dup", STAMP, "dev-a", "gist");
    let written = parse_utc_epoch(STAMP).expect("stamp parses");
    assert_eq!(
        apply_at(&store, &m, written, ENVELOPE_SKEW_DEFAULT_SECS),
        ApplyOutcome::Applied
    );
    assert!(
        store.journal_get("b-dup", "m-dup").unwrap().is_some(),
        "the applied envelope is journaled"
    );
    assert_eq!(
        apply_at(&store, &m, written - 86_400, ENVELOPE_SKEW_DEFAULT_SECS),
        ApplyOutcome::Duplicate,
        "the journal wins over the freshness window"
    );
}
