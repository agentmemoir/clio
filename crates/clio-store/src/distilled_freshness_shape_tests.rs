// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shape of the freshness answer itself: the
//! three states a read surface renders, and the one guarantee the wave's write
//! depends on — that recording a lapse does not change what a later read decides.
//!
//! ## Owns
//! - The three-state answer shape
//! - Recording a `source_changed` lapse, and its refusal to downgrade a reason
//!   already on the record
//!
//! ## Does not own
//! - Which reason applies to which set of sources, the stamp reader, and the
//!   claim-state read (`distilled_freshness_tests`)
//! - Which write path records a lapse (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to the shape of the answer, or to the guarantee that writing a reason
//! is invisible to the next read, should normally require changes here. A
//! scenario for a new set of sources should require
//! `distilled_freshness_tests`.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_freshness::{ClaimedSourceState, UnitFreshness, compute_freshness};
use crate::distilled_unit::{
    DistilledUnitRecord, STALE_REVISION_REJECTED, STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED,
};

/// The instant the fixtures reconcile at, and the instant a lapse is recorded at.
/// A literal in the store's epoch-millis form, like every other stamp in these
/// tests, so nothing here depends on when the suite ran.
const AT: &str = "1767225600000"; // 2026-01-01T00:00:00Z

/// A record reconciled at [`AT`], claiming hub `h` and one source.
fn record() -> DistilledUnitRecord {
    DistilledUnitRecord::new("b", "u", "h", ["s1".to_owned()], "hub_distilled", AT)
}

/// A claimed source still in active use, last changed at the given stamp.
fn active(id: &str, updated_at: &str) -> ClaimedSourceState {
    ClaimedSourceState {
        source_id: id.to_owned(),
        updated_at: Some(updated_at.to_owned()),
        active: true,
    }
}

/// A claimed source whose last change is newer than the reconciliation: the
/// `source_changed` shape.
fn changed(id: &str) -> ClaimedSourceState {
    active(id, "1767229200000") // 2026-01-01T01:00:00Z
}

/// Every claimed source predating the reconciliation: a current unit.
fn current_states() -> Vec<ClaimedSourceState> {
    vec![active("h", AT), active("s1", AT)]
}

/// The three-state answer is what the read surfaces render, so the shape itself
/// is part of the contract: `Some(false)` current, `Some(true)` with a reason,
/// `None` unknown.
#[test]
fn the_freshness_answer_is_a_three_state_with_a_reason_only_when_stale() {
    let current = compute_freshness(&record(), &current_states());
    assert!(matches!(current.stale, Some(false)));
    assert!(current.stale_reason.is_none());

    let stale = compute_freshness(&record(), &[active("h", AT), changed("s1")]);
    assert!(matches!(stale.stale, Some(true)));
    assert!(stale.stale_reason.is_some());

    let unknown = UnitFreshness {
        proof_count: 0,
        last_reconciled_at: AT.to_owned(),
        stale: None,
        stale_reason: None,
    };
    assert!(unknown.stale.is_none());
    assert!(unknown.stale_reason.is_none());
}

#[test]
fn recording_a_source_changed_lapse_leaves_the_read_answer_the_same() {
    // The wave writes the reason it just computed, so the answer a later read
    // gives from the record alone must match the answer the read-time comparison
    // gave. If it did not, writing the reason would change what `get` reports.
    let mut r = record();
    let before = compute_freshness(&r, &[active("h", AT), changed("s1")]);
    assert_eq!(before.stale_reason.as_deref(), Some(STALE_SOURCE_CHANGED));

    assert!(r.mark_source_changed(AT), "a first marking is a change");
    let after = compute_freshness(&r, &current_states());
    assert_eq!(after.stale, before.stale, "{before:?} vs {after:?}");
    assert_eq!(
        after.stale_reason, before.stale_reason,
        "{before:?} vs {after:?}"
    );
    assert_eq!(r.stale_at.as_deref(), Some(AT));
}

#[test]
fn recording_a_source_changed_lapse_does_not_downgrade_a_reason_already_there() {
    // A removal withholds the unit and a refusal is its own determination;
    // replacing either with a weaker "something moved" would be a downgrade, and
    // it would also make a withheld unit visible again.
    for reason in [STALE_SOURCE_REMOVED, STALE_REVISION_REJECTED] {
        let mut r = record();
        r.stale_reason = Some(reason.to_owned());
        r.stale_at = Some(AT.to_owned());
        r.stale_detail = Some("detail".to_owned());

        assert!(
            !r.mark_source_changed(AT),
            "{reason} outranks a moved timestamp, so the write does not land"
        );
        assert_eq!(r.stale_reason.as_deref(), Some(reason));
        assert_eq!(r.stale_at.as_deref(), Some(AT));
        assert_eq!(r.stale_detail.as_deref(), Some("detail"));
    }
}
