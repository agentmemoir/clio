// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-time freshness decision: a current
//! unit, a source that changed, a source that left active use, a stored stale
//! state, an unreadable stamp, and the metadata read that feeds the decision on
//! both backends.
//!
//! ## Owns
//! - The freshness scenarios: current, `source_changed`, `source_removed`,
//!   `revision_rejected`, unknown, and the claim-state read
//!
//! ## Does not own
//! - The answer's own shape and the guarantee that recording a lapse leaves the
//!   next read's answer alone (`distilled_freshness_shape_tests`)
//! - The evidence trail (`distilled_evidence_tests`) and the record SQL
//!   (`distilled_unit_tests`)
//! - The read surfaces that render freshness (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how staleness is decided should normally require changes here.
//! Changes to the answer's shape should require `distilled_freshness_shape_tests`;
//! changes to the trail should require `distilled_evidence_tests`; changes to
//! rendering should require `clio-mcp`.
//!
//! Keep this module focused on the freshness decision.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_freshness::{ClaimedSourceState, compute_freshness, unit_freshness};
use crate::distilled_unit::{
    DistilledUnitRecord, STALE_REVISION_REJECTED, STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED,
};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::MemoryItem;

/// The instant every scenario reconciles at. Written as epoch millis because the
/// maintenance path stamps that form.
const RECONCILED_AT: &str = "1767225600000"; // 2026-01-01T00:00:00Z
/// The same instant in the other form a writer uses, so a mixed-form comparison
/// is exercised rather than assumed.
const RECONCILED_AT_ISO: &str = "2026-01-01T00:00:00Z";

/// A record reconciled at `RECONCILED_AT`, claiming hub `h` and `sources`.
fn record(sources: &[&str]) -> DistilledUnitRecord {
    DistilledUnitRecord::new(
        "b",
        "u",
        "h",
        sources.iter().map(|s| (*s).to_owned()),
        "hub_distilled",
        RECONCILED_AT,
    )
}

/// A source still in active use, last changed at the given stamp.
fn active(id: &str, updated_at: &str) -> ClaimedSourceState {
    ClaimedSourceState {
        source_id: id.to_owned(),
        updated_at: Some(updated_at.to_owned()),
        active: true,
    }
}

/// A source that left active use: the cascade's view of the world.
fn removed(id: &str) -> ClaimedSourceState {
    ClaimedSourceState {
        source_id: id.to_owned(),
        updated_at: Some(RECONCILED_AT.to_owned()),
        active: false,
    }
}

/// A source that is gone entirely: no row, so no stamp and not active.
fn gone(id: &str) -> ClaimedSourceState {
    ClaimedSourceState {
        source_id: id.to_owned(),
        updated_at: None,
        active: false,
    }
}

/// A source whose last change is after the reconciliation: the unit summarizes
/// something that has since moved.
fn changed(id: &str) -> ClaimedSourceState {
    ClaimedSourceState {
        source_id: id.to_owned(),
        updated_at: Some("1767312000000".to_owned()), // 2026-01-02
        active: true,
    }
}

/// Every source unchanged since the reconciliation: the unit is current.
fn current_states() -> Vec<ClaimedSourceState> {
    vec![
        active("h", RECONCILED_AT),
        active("s1", "1767139200000"), // before
        active("s2", RECONCILED_AT),   // exactly at
    ]
}

#[test]
fn a_unit_whose_sources_all_predate_its_reconciliation_is_current() {
    let f = compute_freshness(&record(&["s1", "s2"]), &current_states());
    assert_eq!(f.stale, Some(false), "{f:?}");
    assert_eq!(f.stale_reason, None, "a current unit has no reason");
    assert_eq!(f.last_reconciled_at, RECONCILED_AT);
}

#[test]
fn the_proof_count_and_reconciliation_time_come_from_the_record() {
    let mut r = record(&["s1"]);
    r.record_evidence(
        vec![
            crate::distilled_evidence::EvidenceEntry::new("h", Some(RECONCILED_AT)).expect("h"),
            crate::distilled_evidence::EvidenceEntry::new("s1", Some(RECONCILED_AT)).expect("s1"),
        ],
        RECONCILED_AT,
    );
    let f = compute_freshness(&r, &current_states());
    assert_eq!(f.proof_count, 2, "hub plus one source");
    assert_eq!(f.last_reconciled_at, RECONCILED_AT);
}

#[test]
fn a_source_edited_after_the_reconciliation_is_source_changed() {
    let f = compute_freshness(
        &record(&["s1", "s2"]),
        &[
            active("h", RECONCILED_AT),
            active("s1", RECONCILED_AT),
            changed("s2"),
        ],
    );
    assert_eq!(f.stale, Some(true), "{f:?}");
    assert_eq!(f.stale_reason.as_deref(), Some(STALE_SOURCE_CHANGED));
}

#[test]
fn a_source_edited_after_the_reconciliation_is_detected_across_stamp_forms() {
    // The reconciliation is epoch millis and the source was written by the other
    // path, so this only works if both forms are placed on one timeline.
    let mut r = record(&["s1"]);
    r.distilled_at = RECONCILED_AT.to_owned();
    let f = compute_freshness(
        &r,
        &[
            active("h", RECONCILED_AT_ISO),
            active("s1", "2026-01-02T00:00:00Z"),
        ],
    );
    assert_eq!(
        f.stale_reason.as_deref(),
        Some(STALE_SOURCE_CHANGED),
        "2026-01-02 is after 2026-01-01 whichever form each side used: {f:?}"
    );
}

#[test]
fn a_source_that_left_active_use_is_source_removed() {
    let f = compute_freshness(
        &record(&["s1", "s2"]),
        &[
            active("h", RECONCILED_AT),
            removed("s1"),
            active("s2", RECONCILED_AT),
        ],
    );
    assert_eq!(f.stale, Some(true), "{f:?}");
    assert_eq!(f.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
}

#[test]
fn a_source_row_that_is_gone_counts_as_source_removed() {
    let f = compute_freshness(
        &record(&["s1", "s2"]),
        &[
            active("h", RECONCILED_AT),
            gone("s1"),
            active("s2", RECONCILED_AT),
        ],
    );
    assert_eq!(f.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
}

#[test]
fn a_removal_outranks_an_edit_in_the_same_claim_set() {
    // A removal is an instruction that the content leave active use, so it wins
    // the reason slot whatever else moved.
    let f = compute_freshness(
        &record(&["s1", "s2"]),
        &[active("h", RECONCILED_AT), removed("s1"), changed("s2")],
    );
    assert_eq!(f.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
}

#[test]
fn a_stale_state_the_store_already_recorded_is_reported_as_it_stands() {
    for reason in [
        STALE_SOURCE_REMOVED,
        STALE_REVISION_REJECTED,
        STALE_SOURCE_CHANGED,
    ] {
        let mut r = record(&["s1"]);
        r.stale_reason = Some(reason.to_owned());
        r.stale_at = Some(RECONCILED_AT.to_owned());
        let f = compute_freshness(&r, &current_states());
        assert_eq!(f.stale, Some(true), "{reason}");
        assert_eq!(
            f.stale_reason.as_deref(),
            Some(reason),
            "not re-decided on read"
        );
    }
}

#[test]
fn a_stamp_the_store_cannot_place_yields_unknown_not_current() {
    // A source the caller did not report at all.
    let f = compute_freshness(&record(&["s1", "s2"]), &[active("h", RECONCILED_AT)]);
    assert_eq!(
        f.stale, None,
        "an unreported source is not a current unit: {f:?}"
    );
    assert_eq!(f.stale_reason, None);

    // A source whose stamp is text this store does not write.
    let f = compute_freshness(
        &record(&["s1"]),
        &[active("h", RECONCILED_AT), active("s1", "not a timestamp")],
    );
    assert_eq!(
        f.stale, None,
        "an unreadable stamp is not a current unit: {f:?}"
    );

    // A record whose own reconciliation stamp cannot be placed.
    let mut r = record(&["s1"]);
    r.distilled_at = "whenever".to_owned();
    let f = compute_freshness(&r, &current_states());
    assert_eq!(
        f.stale, None,
        "an unreadable reconciliation is unknown: {f:?}"
    );
}

/// An item whose `updated_at` is the ISO form the MCP write path produces, so
/// the read carries both forms into the decision.
fn item(bank: &str, id: &str) -> MemoryItem {
    let mut it = memory_item(bank, id, "subj");
    it.updated_at = RECONCILED_AT_ISO.to_owned();
    it
}

/// The claim-state read answers both halves of the question from metadata only:
/// a live source reports its stamp, a discarded one reports not-active, and an
/// id the bank never held comes back too, so a caller can see it is gone.
#[test]
fn the_claim_state_read_reports_live_discarded_and_absent_sources() {
    let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
    let bank = unique("claim-state");
    let live = unique("live");
    let discarded = unique("discarded");
    let absent = unique("absent");
    store
        .create_memory_item(&item(&bank, &live), "agent")
        .expect("live item");
    store
        .create_memory_item(&item(&bank, &discarded), "agent")
        .expect("discarded item");
    store
        .discard_item(&bank, &discarded, "test", RECONCILED_AT, "agent")
        .expect("discard");

    let asked = vec![live.clone(), discarded.clone(), absent.clone()];
    let states = store
        .distilled_unit_claim_state(&bank, &asked)
        .expect("claim state");
    let by_id: std::collections::BTreeMap<&str, &ClaimedSourceState> =
        states.iter().map(|s| (s.source_id.as_str(), s)).collect();

    let live_state = by_id[live.as_str()];
    assert!(live_state.active, "a stored source is in active use");
    assert_eq!(
        live_state.updated_at.as_deref(),
        Some(RECONCILED_AT_ISO),
        "its stamp comes from the item row"
    );

    let discarded_state = by_id[discarded.as_str()];
    assert!(
        !discarded_state.active,
        "a discarded source is not in active use"
    );

    let absent_state = by_id[absent.as_str()];
    assert!(
        !absent_state.active,
        "an id the bank never held is not active"
    );
    assert_eq!(absent_state.updated_at, None, "and has no stamp");

    assert_eq!(
        states
            .iter()
            .map(|s| s.source_id.as_str())
            .collect::<Vec<_>>(),
        vec![absent.as_str(), discarded.as_str(), live.as_str()],
        "ordered by id, so two reads cannot differ"
    );
}

#[test]
fn the_claim_state_read_of_an_empty_claim_set_is_empty() {
    let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
    let claim_state = store
        .distilled_unit_claim_state("b", &[])
        .expect("empty read");
    assert!(claim_state.is_empty(), "{claim_state:?}");
}

/// The store-calling composition: it reads the record's own claim set and returns
/// the freshness answer a read surface renders.
#[test]
fn the_store_composition_reads_the_claim_set_and_decides_freshness() {
    let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
    let bank = unique("freshness-compose");
    let hub = unique("hub");
    let source = unique("source");
    for id in [&hub, &source] {
        store
            .create_memory_item(&item(&bank, id), "agent")
            .expect("item");
    }
    // The items were stamped before the reconciliation instant below, so the
    // unit reads current without any test waiting on real time.
    let r = DistilledUnitRecord::new(
        &bank,
        "u",
        &hub,
        [source.clone()],
        "hub_distilled",
        "4102444800000", // 2100-01-01
    );
    let f = unit_freshness(&store, &bank, &r).expect("freshness");
    assert_eq!(f.stale, Some(false), "no source moved past 2100: {f:?}");
    assert_eq!(f.last_reconciled_at, "4102444800000");

    // And a source that leaves active use flips it, with no wave involved.
    store
        .discard_item(&bank, &source, "test", "4102444800001", "agent")
        .expect("discard");
    let after = unit_freshness(&store, &bank, &r).expect("freshness");
    assert_eq!(after.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
}
