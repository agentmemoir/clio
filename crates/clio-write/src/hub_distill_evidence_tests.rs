// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-revision evidence a distilled unit
//! carries on the write side: one entry per claimed source on a first
//! distillation and on every reconcile, append-only per revision, and the
//! by-reason count of units a wave left not current.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - The evidence trail after a first distill, after a revision, and after a
//!   refused revision
//! - The report's `stale_units` tally, including the reasons and the fact that
//!   an unknown freshness is never counted as current
//!
//! ## Does not own
//! - The evidence and freshness rules themselves (`clio-store`)
//! - The write paths that record the trail (`hub_distill_model`,
//!   `hub_distill_reconcile`) and the report shape (`hub_distill_model`)
//! - The ordinary reconcile wave scenarios (`hub_distill_reconcile_tests`), and
//!   the no-op wave's trail assertion, which stays beside the record test it
//!   extends (`hub_distill_record_tests`)
//!
//! ## Boundary
//!
//! Changes to what a wave must record as proof of a unit should normally
//! require changes here. Changes to how staleness is decided should not require
//! changes here.
//!
//! Keep this module focused on the evidence trail and the freshness tally.

use super::hub_distill_reconcile_tests::{
    BANK, HUB, UNIT, add_source, discard_item, first_wave, record_of, seeded, wave,
};
use super::hub_distill_tests::{WAVE_CLOCK_STAMP, ctx, engine_at, semantic};
use crate::hub_distill::StaleTally;
use crate::hub_distill_model::claimed_evidence;
use clio_config::retention::ResolvedRetention;
use clio_store::{STALE_SOURCE_CHANGED, SqliteStore, Store};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Every seeded item carries the wave clock's own millisecond (`seed_hub` pins
/// it), so a unit is current for the reason a unit is current rather than
/// because of how the fixture was built.
const WAVE_STAMP: &str = WAVE_CLOCK_STAMP;

/// One millisecond after [`WAVE_STAMP`]: the stamp that says "this source
/// changed after the unit's last reconciliation" without moving the wave clock.
const CHANGED_STAMP: &str = "1";

/// The seeded cluster, with pruning disabled so the hub stays eligible and its
/// edges stay readable for later reconciles.
fn seeded_at_the_wave_clock() -> (Arc<SqliteStore>, crate::hub_distill::HubDistillEngine) {
    seeded()
}

/// The source ids of an entry list, in the order the caller is given them.
fn entry_ids(entries: &[clio_store::EvidenceEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.source_id.as_str())
        .collect()
}

#[test]
fn a_first_distillation_records_one_entry_per_claimed_source() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    let record = record_of(&store, UNIT);

    let trail = record.evidence_history();
    assert_eq!(trail.len(), 1, "one list per revision: {trail:?}");
    assert_eq!(trail[0].revision, 1);
    assert_eq!(
        trail[0].reconciled_at, "1970-01-01T00:00:00Z",
        "the list carries the wave's own reconciliation stamp"
    );
    assert_eq!(
        entry_ids(&trail[0].entries),
        vec![HUB, "n1", "n2", "n3"],
        "one entry per claimed source, hub included, stable by id"
    );
    // The entries carry the sources' own stamps. This wave's clock is pinned at
    // the same millisecond, so the two are equal here and cannot be told apart —
    // which is why the revision test below asserts the same thing across two
    // DIFFERENT stamps, where a list that took the wave's time would show it.
    assert!(
        trail[0]
            .entries
            .iter()
            .all(|entry| entry.as_of.as_deref() == Some(WAVE_STAMP)),
        "each entry carries the source's own last-change time: {:?}",
        trail[0]
    );
    assert_eq!(record.proof_count(), 4);
    assert_eq!(
        record.proof_count(),
        record.claim_ids().len(),
        "the proof count is the claim set's distinct sources"
    );
    assert_eq!(record.current_evidence(), &trail[0].entries[..]);
}

#[test]
fn a_revision_appends_its_own_list_and_keeps_the_prior_one() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    let before = record_of(&store, UNIT);
    add_source(&store, "n4");
    // A later wave clock, so each list carries its own reconciliation time and
    // a shared stamp could not pass for one.
    let later = engine_at(&store, 0.0, 2, 1000);
    let report = later
        .consolidate_hubs(BANK, &ctx(BANK), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");

    let after = record_of(&store, UNIT);
    assert_eq!(after.revision, 2);
    let trail = after.evidence_history();
    assert_eq!(trail.len(), 2, "one list per revision, appended");
    assert_eq!(
        trail[0],
        before.evidence_history()[0],
        "the prior revision's list is unchanged and still readable"
    );
    assert_eq!(trail[1].revision, 2);
    assert_eq!(trail[1].reconciled_at, "1970-01-01T00:00:01Z");
    assert_eq!(
        entry_ids(&trail[1].entries),
        vec![HUB, "n1", "n2", "n3", "n4"]
    );
    // The sources were stamped at millis 0 and this wave reconciles at 1000, so
    // the two stamps differ here and an entry that took the wave's time instead
    // of the source's would be visible.
    assert!(
        trail[1]
            .entries
            .iter()
            .all(|entry| entry.as_of.as_deref() == Some(WAVE_STAMP)),
        "each entry carries the source's own last-change time, not the wave's: {:?}",
        trail[1]
    );
    assert_eq!(
        trail.iter().map(|list| list.revision).collect::<Vec<_>>(),
        vec![1, 2],
        "revisions are strictly increasing, so no two lists claim one revision"
    );
    assert_eq!(
        after.proof_count(),
        5,
        "the proof count is the current revision's list"
    );
}

#[test]
fn a_source_named_twice_yields_one_entry() {
    // The same item twice is what a claim set that named the hub as its own
    // neighbour hands this rule, and what a retried write hands it.
    let hub = semantic(HUB, BANK, "hub core");
    let n1 = semantic("n1", BANK, "neighbor n1");
    let entries = claimed_evidence(&hub, &[n1.clone(), n1, hub.clone()]);

    assert_eq!(entry_ids(&entries), vec![HUB, "n1"]);
    let distinct: BTreeSet<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    assert_eq!(
        distinct.len(),
        entries.len(),
        "no source id repeats inside one revision"
    );
}

#[test]
fn a_refused_revision_records_no_evidence() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    let before = record_of(&store, UNIT);
    add_source(&store, "n4");
    // A stricter policy refuses the re-derived revision.
    let mut strict = engine_at(&store, 1.0, 2, 0);
    strict.set_prune_after_distill(false);

    let report = wave(&strict);
    assert_eq!(
        report.reconcile_rejected,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    let after = record_of(&store, UNIT);
    assert_eq!(after.revision, 1, "the refused revision replaced nothing");
    assert_eq!(
        after.evidence_history(),
        before.evidence_history(),
        "a revision that was not admitted did not happen, so it left no evidence"
    );
    assert_eq!(after.proof_count(), before.proof_count());

    // A later wave under the permissive policy does admit it, and then records.
    let admitted = wave(&engine);
    assert_eq!(admitted.revised, vec![HUB.to_owned()], "{admitted:?}");
    let record = record_of(&store, UNIT);
    assert_eq!(record.evidence_history().len(), 2);
    // A stale unit rebuilds from the claim set its record holds, and the
    // refused revision never recorded `n4`, so revision 2 claims the hub and
    // its three recorded neighbours — the evidence list and the claim set are
    // the same set.
    let current = record.current_evidence();
    assert_eq!(current.len(), 4, "hub plus the three recorded neighbours");
    assert_eq!(entry_ids(current), vec![HUB, "n1", "n2", "n3"]);
    assert_eq!(record.proof_count(), record.claim_ids().len());
}

#[test]
fn a_wave_that_makes_a_unit_current_does_not_count_it_as_stale() {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    add_source(&store, "n4");

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");
    assert_eq!(
        report.stale_units,
        StaleTally::default(),
        "a unit this wave made current is current, whatever the store says"
    );
    assert_eq!(report.stale_units.total(), 0);
    assert_eq!(record_of(&store, UNIT).stale_reason, None);
}

#[test]
fn a_refused_revision_is_counted_under_its_own_reason() {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    add_source(&store, "n4");
    let mut strict = engine_at(&store, 1.0, 2, 0);
    strict.set_prune_after_distill(false);

    let report = wave(&strict);
    assert_eq!(
        report.reconcile_rejected,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    assert_eq!(
        report.stale_units,
        StaleTally {
            revision_rejected: 1,
            ..StaleTally::default()
        },
        "a unit that is not current is counted under its own reason, never as current"
    );
    assert_eq!(report.stale_units.total(), 1);
    assert_eq!(
        record_of(&store, UNIT).stale_reason.as_deref(),
        Some("revision_rejected")
    );
}

#[test]
fn a_unit_whose_source_left_active_use_is_counted_under_its_own_reason() {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    // The hub itself leaves active use, so no reconcile can rebuild the unit.
    discard_item(&store, HUB);

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    assert_eq!(
        report.stale_units,
        StaleTally {
            source_removed: 1,
            ..StaleTally::default()
        },
        "freshness and the wave's own outcome describe different things, so a \
         unit in `reconcile_stale_kept` is counted here as well"
    );
    assert_eq!(report.stale_units.total(), 1);
}

#[test]
fn a_unit_whose_source_changed_is_counted_and_the_reason_is_written_although_the_wave_found_no_change()
 {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    // A source edited after the reconciliation: its last-change time moves and
    // its text does not, so the wave's own text comparison finds nothing to
    // re-derive and the record's reconciliation stamp stays where it was.
    let mut edited = store.get_memory_item(BANK, "n1").unwrap().unwrap();
    edited.updated_at = CHANGED_STAMP.into();
    store.update_memory_item(&edited, "agent").unwrap();

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_unchanged,
        vec![HUB.to_owned()],
        "the wave's text comparison sees no change: {report:?}"
    );
    assert!(report.revised.is_empty(), "revised: {:?}", report.revised);
    assert_eq!(
        report.stale_units,
        StaleTally {
            source_changed: 1,
            ..StaleTally::default()
        },
        "a source newer than the reconciliation is reported even when the wave \
         compared only the text and found it equal"
    );
    assert_eq!(report.stale_units.total(), 1);

    // The reason is written too. Recall reads the item's snapshot and nothing
    // else, so a reason that stayed only in the freshness comparison would leave
    // recall treating the unit as a current parent.
    let record = record_of(&store, UNIT);
    assert_eq!(record.stale_reason.as_deref(), Some(STALE_SOURCE_CHANGED));
    assert_eq!(
        record.stale_at.as_deref(),
        Some(WAVE_STAMP),
        "the wave's own clock says when it recorded the lapse"
    );
    assert!(
        record.stale_detail.is_none(),
        "an edit is not a determination with a detail: {:?}",
        record.stale_detail
    );
    let item = store
        .get_memory_item(BANK, UNIT)
        .unwrap()
        .expect("the unit's item row");
    assert_eq!(
        item.snapshot
            .as_ref()
            .and_then(|s| s.get("stale_reason"))
            .and_then(|r| r.as_str()),
        Some(STALE_SOURCE_CHANGED),
        "and the snapshot recall reads carries it: {:?}",
        item.snapshot
    );
}

#[test]
fn a_second_wave_over_an_already_marked_unit_rewrites_nothing() {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    let mut edited = store.get_memory_item(BANK, "n1").unwrap().unwrap();
    edited.updated_at = CHANGED_STAMP.into();
    store.update_memory_item(&edited, "agent").unwrap();
    wave(&engine);
    let marked = record_of(&store, UNIT);

    let again = wave(&engine);
    assert_eq!(
        record_of(&store, UNIT),
        marked,
        "the reason is already there, so a second wave over the same lapse \
         changes nothing: {again:?}"
    );
}

#[test]
fn a_successful_revision_clears_a_wave_recorded_source_changed_lapse() {
    let (store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    let mut edited = store.get_memory_item(BANK, "n1").unwrap().unwrap();
    edited.updated_at = CHANGED_STAMP.into();
    store.update_memory_item(&edited, "agent").unwrap();
    wave(&engine);
    assert_eq!(
        record_of(&store, UNIT).stale_reason.as_deref(),
        Some(STALE_SOURCE_CHANGED)
    );
    // A neighbour joins, so the next wave's comparison finds a claim-set change
    // and re-derives — the wave that rebuilds is what clears the lapse, not the
    // pass that recorded it. Its clock is past the edited source's stamp, because
    // a revision only reconciles what it re-read: a wave at millis 0 would rebuild
    // from a source stamped at millis 1 and record the same lapse again, which is
    // the honest answer for a wave that ran before the edit.
    add_source(&store, "n4");
    let later = engine_at(&store, 0.0, 2, 2000);
    let report = later
        .consolidate_hubs(BANK, &ctx(BANK), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");

    let record = record_of(&store, UNIT);
    assert_eq!(
        record.stale_reason, None,
        "a unit whose claim set was re-derived is current again"
    );
    assert_eq!(record.stale_at, None);
    assert_eq!(record.stale_detail, None);
    assert_eq!(
        report.stale_units,
        StaleTally::default(),
        "and the wave that cleared it reports the bank as current: {report:?}"
    );
}

#[test]
fn the_summary_keeps_its_parsed_keys_and_appends_the_freshness_counts() {
    let (_store, mut engine) = seeded_at_the_wave_clock();
    first_wave(&mut engine);
    let summary = wave(&engine).summary();
    let fields: Vec<&str> = summary
        .trim_start_matches("hub_distill: ")
        .split(',')
        .collect();
    let names: Vec<&str> = fields
        .iter()
        .map(|field| field.split('=').next().unwrap())
        .collect();

    // Existing readers split this string and look a key up by prefix, so the
    // keys they know keep both their names and their order, and the new counts
    // are appended rather than inserted among them.
    assert_eq!(
        names,
        vec![
            "eligible",
            "distilled",
            "rejected",
            "existing",
            "revised",
            "revised_rejected",
            "unchanged",
            "stale_kept",
            "stale",
            "stale_source_removed",
            "stale_source_changed",
            "stale_revision_rejected",
            "stale_unknown",
        ],
        "summary: {summary}"
    );
    assert_eq!(fields[1], "distilled=0", "summary: {summary}");
    assert_eq!(fields[8], "stale=0", "summary: {summary}");
}
