// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconcile pass's handling of a unit it
//! must leave alone: one whose hub left active use, one a sibling shares a
//! subject with, one whose content erasure destroyed, and one an operator
//! correction superseded.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - A unit whose hub left active use staying stale and readable, by discard and
//!   by a missing hub row
//! - A sibling sharing the hub's subject surviving a revision
//! - A unit without content reported as purged rather than resurrected
//! - A unit superseded by an operator correction reported, history untouched
//!
//! ## Does not own
//! - The ordinary reconcile scenarios — a changed unit re-derived, an unchanged
//!   one left alone, a refusal, a rebuild after a discard
//!   (`hub_distill_reconcile_tests`)
//! - The edge-conflict scenarios (`hub_distill_reconcile_edge_tests`)
//! - The reconcile decision and its outcomes (`hub_distill_reconcile`)
//!
//! ## Boundary
//!
//! Changes to what the reconcile pass does with a unit it must not rebuild
//! should normally require changes here. Changes to what it does with a unit it
//! should rebuild should require `hub_distill_reconcile_tests`.
//!
//! Keep this module focused on the units a wave leaves alone.

use super::hub_distill_reconcile_tests::{
    BANK, HUB, REV2, UNIT, add_source, discard_item, first_wave, record_of, seeded, wave,
};
use super::hub_distill_tests::{WAVE_CLOCK_STAMP, engine, semantic, store};
use clio_store::{STALE_SOURCE_REMOVED, Store};

#[test]
fn a_unit_whose_hub_left_active_use_stays_stale_and_readable() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    // The ordinary route: the hub is discarded. The cascade marks the unit
    // stale, because the hub is part of its claim set, and nothing a reconcile
    // does can rebuild a unit whose hub left active use.
    discard_item(&store, HUB);
    assert_eq!(
        record_of(&store, UNIT).stale_reason.as_deref(),
        Some(STALE_SOURCE_REMOVED),
        "the cascade marked the unit"
    );

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    let record = record_of(&store, UNIT);
    assert_eq!(
        record.stale_reason.as_deref(),
        Some(STALE_SOURCE_REMOVED),
        "a wave must not clear a removal"
    );
    assert!(
        store
            .get_memory_item(BANK, &record.current_revision_item_id())
            .unwrap()
            .is_some(),
        "the unit stays readable"
    );
    assert!(
        report.revised.is_empty(),
        "no rebuild may resurrect a discarded hub's unit"
    );

    // The degenerate route: the hub row is gone entirely, its edges with it. The
    // hub is then not eligible and there is nothing left to distill, so the
    // reconcile pass - not a distill attempt - is what reaches the missing row,
    // and it reports the unit the same way.
    for edge in store.list_assoc_edges(BANK, Some(HUB)).unwrap() {
        store.delete_assoc_edge(BANK, &edge.id).unwrap();
    }
    store.delete_item(BANK, HUB).unwrap();
    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "{report:?}"
    );
}

#[test]
fn a_sibling_sharing_the_hubs_subject_is_not_closed_by_a_revision() {
    let store = store();
    // The hub and a sibling share one data subject: the shape a `(bank,
    // subject)` lineage lookup would resolve, and close, by mistake.
    for id in [HUB, "sibling", "n1", "n2", "n3"] {
        let mut item = semantic(id, BANK, &format!("{id} core"));
        item.subject_id = "shared-subject".into();
        item.updated_at = WAVE_CLOCK_STAMP.to_owned();
        store.create_memory_item(&item, "agent").unwrap();
    }
    for neighbor in ["n1", "n2", "n3"] {
        store
            .reinforce_assoc_edge(BANK, HUB, neighbor, 0, 0.15)
            .expect("reinforce");
    }
    let mut engine = engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    first_wave(&mut engine);
    add_source(&store, "n4");

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);
    // The revision closed only the unit's own prior revision.
    let sibling = store.get_memory_item(BANK, "sibling").unwrap().unwrap();
    assert_eq!(sibling.tx_until, None, "the sibling must not be closed");
    assert!(
        store.get_distilled_unit(BANK, "sibling").unwrap().is_none(),
        "no record is keyed by the sibling's id"
    );
}

#[test]
fn the_reconcile_pass_reports_a_unit_without_content_as_purged() {
    // A unit whose content erasure destroyed is reported, never resurrected:
    // the record row survives the purge, but reconciliation leaves it alone.
    // Erasure nulls the sealed content; deleting the current revision row
    // stands in for that state here (the record key is the unit id, not the
    // revision id, so the record survives the delete).
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    add_source(&store, "n4");
    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);
    store.delete_item(BANK, REV2).unwrap();
    assert!(
        store.get_distilled_unit(BANK, UNIT).unwrap().is_some(),
        "the record survives the revision row's delete"
    );

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    assert!(report.revised.is_empty(), "revised: {:?}", report.revised);
}

#[test]
fn a_wave_after_an_operator_correction_reports_the_unit_superseded() {
    // The operator `correct`s the unit's current revision: the correction
    // closes it on both axes and the record no longer tracks the unit's
    // text. A later wave must not error and must not rewrite history.
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    add_source(&store, "n4");
    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);

    let rev2 = store.get_memory_item(BANK, REV2).unwrap().unwrap();
    let ts = "2026-06-05T00:00:00Z";
    let mut corrected = rev2;
    corrected.id = "hub-distill:bank:hub-corrected".into();
    corrected.gist = Some("the corrected summary text".into());
    corrected.valid_from = ts.into();
    corrected.tx_from = ts.into();
    corrected.created_at = ts.into();
    corrected.updated_at = ts.into();
    let outcome = store
        .correct_item(&clio_store::CorrectionSpec {
            bank_id: BANK.into(),
            prior_item_id: REV2.into(),
            new_item: corrected,
            reason: "the summary misstated the hub".into(),
            actor: "operator".into(),
            timestamp: ts.into(),
        })
        .unwrap();
    assert!(!outcome.already_superseded);

    // The wave reports the superseded unit instead of fighting the correction.
    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    assert!(report.revised.is_empty(), "revised: {:?}", report.revised);
    // The correction's rows are untouched by the wave.
    let corrected_row = store
        .get_memory_item(BANK, "hub-distill:bank:hub-corrected")
        .unwrap()
        .unwrap();
    assert_eq!(corrected_row.tx_until, None);
    let rev2_row = store.get_memory_item(BANK, REV2).unwrap().unwrap();
    assert!(
        rev2_row.tx_until.is_some(),
        "the superseded row stays closed"
    );
}
