// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the stale-unit recall contract tests: a unit
//! stale for source removal is withheld from `consolidated_only` and does not
//! suppress a raw item under `prefer_consolidated`, while any other stale
//! reason stays current.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The scope filter and parent-cap selection (`dedup`, `dedup_cap`)
//! - The hit shape and its contracts (`clio-retrieve`)
//!
//! ## Boundary
//!
//! Changes to which stale units are withheld from consolidated eligibility
//! should normally require changes here. Changes to the suppression or
//! backfill algorithm should not require changes here.
//!
//! Keep this module focused on the stale-unit recall contracts.

use super::dedup_tests::{cfg, hit, ids, map, raw, run};
use super::{
    DedupConfig, HUB_DISTILL_SOURCE_REF, STALE_SOURCE_REMOVED, is_consolidated_item,
    is_source_removed_stale, is_stale,
};
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_types::MemoryItem;
use serde_json::json;

/// A consolidated parent carrying the given `stale_reason`, when any.
fn parent(id: &str, hub: &str, neighbors: &[&str], stale_reason: Option<&str>) -> MemoryItem {
    let mut item = raw(id, "parent summary");
    item.source_ref = Some(HUB_DISTILL_SOURCE_REF.into());
    let mut snapshot = json!({
        "hub_id": hub,
        "neighbor_ids": neighbors,
        "neighbor_count": neighbors.len(),
    });
    if let Some(reason) = stale_reason {
        snapshot["stale_reason"] = json!(reason);
    }
    item.snapshot = Some(snapshot);
    item
}

fn only_scope() -> DedupConfig {
    cfg(
        RecallScope::ConsolidatedOnly,
        false,
        DuplicateTolerance::Balanced,
    )
}

fn prefer_scope() -> DedupConfig {
    cfg(RecallScope::Full, true, DuplicateTolerance::Balanced)
}

#[test]
fn a_source_removed_unit_is_withheld_from_consolidated_only() {
    let items = map(vec![
        parent("U", "HUB", &["A"], Some(STALE_SOURCE_REMOVED)),
        raw("A", "raw alpha"),
    ]);
    let out = run(vec![hit("U", 2.0), hit("A", 1.0)], &items, 10, only_scope());
    assert_eq!(ids(&out), Vec::<String>::new());
    assert!(out.dedup.scope_empty);
}

#[test]
fn a_current_sibling_is_still_returned_by_consolidated_only() {
    let items = map(vec![
        parent("U", "HUB", &["A"], Some(STALE_SOURCE_REMOVED)),
        parent("V", "HUB", &["A"], None),
        raw("A", "raw alpha"),
    ]);
    let out = run(
        vec![hit("U", 2.0), hit("V", 1.5), hit("A", 1.0)],
        &items,
        10,
        only_scope(),
    );
    assert_eq!(ids(&out), vec!["V".to_owned()]);
    assert!(!out.dedup.scope_empty);
}

#[test]
fn a_source_removed_parent_does_not_suppress_its_raw_item() {
    let items = map(vec![
        parent("U", "HUB", &["A"], Some(STALE_SOURCE_REMOVED)),
        raw("A", "raw alpha"),
    ]);
    let out = run(
        vec![hit("U", 2.0), hit("A", 1.0)],
        &items,
        10,
        prefer_scope(),
    );
    assert_eq!(ids(&out), vec!["U".to_owned(), "A".to_owned()]);
    assert!(
        out.dedup.suppressed_raw_ids.is_empty(),
        "{:?}",
        out.dedup.suppressed_raw_ids
    );
}

#[test]
fn a_current_parent_still_suppresses_its_raw_item() {
    let items = map(vec![
        parent("V", "HUB", &["A"], None),
        raw("A", "raw alpha"),
    ]);
    let out = run(
        vec![hit("V", 2.0), hit("A", 1.0)],
        &items,
        10,
        prefer_scope(),
    );
    assert_eq!(ids(&out), vec!["V".to_owned()]);
    assert_eq!(out.dedup.suppressed_raw_ids, vec!["A".to_owned()]);
}

#[test]
fn a_source_removed_unit_is_still_returned_under_the_full_scope() {
    let items = map(vec![parent("U", "HUB", &["A"], Some(STALE_SOURCE_REMOVED))]);
    let out = run(
        vec![hit("U", 2.0)],
        &items,
        10,
        cfg(RecallScope::Full, false, DuplicateTolerance::Balanced),
    );
    assert_eq!(ids(&out), vec!["U".to_owned()]);
    assert!(!out.dedup.scope_empty);
}

#[test]
fn only_the_source_removed_reason_withholds() {
    // `revision_rejected` and an unknown reason are display-only: the unit is
    // returned by `consolidated_only`.
    for reason in ["revision_rejected", "something_else"] {
        let items = map(vec![parent("U", "HUB", &["A"], Some(reason))]);
        let out = run(vec![hit("U", 2.0)], &items, 10, only_scope());
        assert_eq!(ids(&out), vec!["U".to_owned()], "reason: {reason}");
        assert!(!out.dedup.scope_empty, "reason: {reason}");
    }
    // A unit with no stale state at all stays current.
    let items = map(vec![parent("U", "HUB", &["A"], None)]);
    let out = run(vec![hit("U", 2.0)], &items, 10, only_scope());
    assert_eq!(ids(&out), vec!["U".to_owned()]);
}

#[test]
fn the_stale_predicate_reads_only_the_snapshot_marker() {
    let unit = parent("U", "HUB", &["A"], Some(STALE_SOURCE_REMOVED));
    assert!(is_consolidated_item(&unit));
    assert!(is_source_removed_stale(&unit));
    assert!(!is_source_removed_stale(&parent("V", "HUB", &["A"], None)));
    assert!(!is_source_removed_stale(&raw("A", "raw alpha")));

    // The two predicates answer different questions, and only the removal one
    // withholds. A reason recall has never heard of is still "stale" for
    // suppression, so an unknown reason cannot quietly make a lapsed parent
    // suppress the item it may no longer represent.
    assert!(is_stale(&parent(
        "U",
        "HUB",
        &["A"],
        Some("source_removed")
    )));
    assert!(is_stale(&parent(
        "U",
        "HUB",
        &["A"],
        Some("revision_rejected")
    )));
    assert!(is_stale(&parent(
        "U",
        "HUB",
        &["A"],
        Some("something_else")
    )));
    assert!(!is_stale(&parent("V", "HUB", &["A"], None)));
    assert!(!is_stale(&parent("W", "HUB", &["A"], Some(""))));
    assert!(!is_stale(&raw("A", "raw alpha")));
}

/// One bank, all three stale reasons side by side, both scopes. The two
/// treatments are the whole point of the split, so they are pinned together:
/// a removal withholds, and nothing else does, in the same page.
///
/// The reasons are named as literals rather than imported, because a stale
/// reason this read side does not hold a copy of must still behave as
/// display-only: recall's contract is that only `source_removed` withholds, and a
/// rename on either side must not quietly change which reasons hide a claim.
#[test]
fn one_bank_with_all_three_reasons_returns_two_and_withholds_one() {
    let items = map(vec![
        parent("REMOVED", "H1", &["A"], Some("source_removed")),
        parent("CHANGED", "H2", &["B"], Some("source_changed")),
        parent("REJECTED", "H3", &["C"], Some("revision_rejected")),
        raw("A", "raw alpha"),
        raw("B", "raw beta"),
        raw("C", "raw gamma"),
    ]);

    // `consolidated_only` returns the two non-removal reasons and withholds the
    // removal, from the same bank in the same call.
    let out = run(
        vec![
            hit("REMOVED", 5.0),
            hit("CHANGED", 4.0),
            hit("REJECTED", 3.0),
            hit("A", 2.0),
            hit("B", 1.0),
            hit("C", 0.5),
        ],
        &items,
        10,
        only_scope(),
    );
    let mut returned = ids(&out);
    returned.sort();
    assert_eq!(
        returned,
        vec!["CHANGED".to_owned(), "REJECTED".to_owned()],
        "only source_removed withholds"
    );
    assert!(!out.dedup.scope_empty);

    // Under `prefer_consolidated`, the returned parents suppress their own raw
    // sources and the withheld one does not suppress anything: it is not even on
    // the page, so a raw item it may no longer represent is still visible.
    let out = run(
        vec![
            hit("CHANGED", 4.0),
            hit("REJECTED", 3.0),
            hit("A", 2.0),
            hit("B", 1.5),
            hit("C", 1.0),
        ],
        &items,
        10,
        prefer_scope(),
    );
    let mut returned = ids(&out);
    returned.sort();
    assert_eq!(
        returned,
        vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "CHANGED".to_owned(),
            "REJECTED".to_owned()
        ],
        "every raw item survives: one because its parent is withheld, two because \
         their parents are returned but no longer current"
    );
    let mut suppressed = out.dedup.suppressed_raw_ids.clone();
    suppressed.sort();
    assert_eq!(
        suppressed,
        Vec::<String>::new(),
        "no stale unit suppresses anything: A's parent is withheld, and B and C \
         belong to parents that are returned but no longer current"
    );
}

/// A stale-but-returned unit does not suppress the raw items it claims, because
/// it may no longer represent them. A current unit does. This is the difference
/// between the two recall treatments in one assertion each.
#[test]
fn only_a_current_unit_suppresses_the_items_it_claims() {
    let hits = vec![hit("U", 2.0), hit("A", 1.0)];
    let raw = raw("A", "raw alpha");

    let current = map(vec![parent("U", "HUB", &["A"], None), raw.clone()]);
    let out = run(hits.clone(), &current, 10, prefer_scope());
    assert_eq!(ids(&out), vec!["U".to_owned()], "the parent wins the page");
    assert_eq!(
        out.dedup.suppressed_raw_ids,
        vec!["A".to_owned()],
        "a current unit suppresses what it claims"
    );

    for reason in ["source_removed", "source_changed", "revision_rejected"] {
        let stale = map(vec![parent("U", "HUB", &["A"], Some(reason)), raw.clone()]);
        let out = run(hits.clone(), &stale, 10, prefer_scope());
        assert_eq!(
            ids(&out),
            vec!["U".to_owned(), "A".to_owned()],
            "a stale unit is returned and its raw source is not hidden: {reason}"
        );
        assert!(
            out.dedup.suppressed_raw_ids.is_empty(),
            "nothing is suppressed for {reason}: {out:?}"
        );
    }
}

/// A `source_changed` unit is display-only: it is returned by `consolidated_only`
/// and it suppresses nothing, exactly as a unit stale for any other reason is.
/// A current unit is the only one that suppresses.
#[test]
fn a_source_changed_unit_is_returned_and_suppresses_nothing() {
    let changed = map(vec![
        parent("U", "HUB", &["A"], Some("source_changed")),
        raw("A", "raw alpha"),
    ]);
    let out = run(
        vec![hit("U", 2.0), hit("A", 1.0)],
        &changed,
        10,
        only_scope(),
    );
    assert_eq!(ids(&out), vec!["U".to_owned()], "returned by the scope");
    assert!(!out.dedup.scope_empty);

    let out = run(
        vec![hit("U", 2.0), hit("A", 1.0)],
        &changed,
        10,
        prefer_scope(),
    );
    assert_eq!(
        ids(&out),
        vec!["U".to_owned(), "A".to_owned()],
        "returned, and the raw source it may no longer represent is not hidden"
    );
    assert!(
        out.dedup.suppressed_raw_ids.is_empty(),
        "{:?}",
        out.dedup.suppressed_raw_ids
    );
}
