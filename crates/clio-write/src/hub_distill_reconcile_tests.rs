// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconciliation engine tests a wave must
//! rebuild a unit for: a changed unit re-derived, an unchanged one left alone, a
//! revision preserving the prior version and re-entering admission, a refusal
//! keeping the prior, and a rebuild after a discard without the removed source.
//! It also owns the shared fixture the other reconcile test modules build on.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - The scenarios above, and the fixture: `seeded`, `first_wave`, `wave`,
//!   `discard_item`, `add_source`, `record_of`, and the `BANK`/`HUB`/`UNIT`/
//!   `REV2` ids
//!
//! ## Does not own
//! - The units a wave must leave alone (`hub_distill_reconcile_lifecycle_tests`)
//! - The edge-conflict scenarios (`hub_distill_reconcile_edge_tests`)
//! - The reconcile decision and its outcomes (`hub_distill_reconcile`)
//! - The wave envelope (`hub_distill`) or the scans (`hub_distill_scan`)
//! - The store revision write and cascade (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to what a wave must rebuild for a known distilled unit should normally
//! require changes here, and changes to the shared fixture belong here too.
//! Changes to a unit a wave leaves alone should require
//! `hub_distill_reconcile_lifecycle_tests`; changes to how a unit is stored
//! should not require changes here.
//!
//! Keep this module focused on the reconcile wave scenarios.

use crate::discard::{DiscardArgs, DiscardVecSink, discard};
use crate::hub_distill::HUB_DISTILL_MARKER;
use crate::hub_distill::HubDistillEngine;
use crate::hub_distill::hub_distill_tests::{
    WAVE_CLOCK_STAMP, ctx, engine, seed_hub, semantic, store,
};
use crate::hub_distill_model::HubDistillReport;
use clio_config::retention::ResolvedRetention;
use clio_store::{AssocKind, Store};
use std::sync::Arc;

pub(super) const BANK: &str = "bank";
pub(super) const HUB: &str = "hub";
pub(super) const UNIT: &str = "hub-distill:bank:hub";
pub(super) const REV2: &str = "hub-distill:bank:hub:v2";

/// A seeded cluster with its engine: pruning disabled so the hub stays
/// eligible and the cluster's edges stay readable for later reconciles.
pub(super) fn seeded() -> (Arc<clio_store::SqliteStore>, HubDistillEngine) {
    let store = store();
    seed_hub(&store, BANK, HUB, &["n1", "n2", "n3"], 0.15);
    let mut engine = engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    (store, engine)
}

/// The first wave distills the cluster; asserts the basic outcome.
pub(super) fn first_wave(engine: &mut HubDistillEngine) {
    let report = wave(engine);
    assert_eq!(report.distilled, vec![UNIT.to_owned()], "{report:?}");
    assert!(
        report.rejected.is_empty(),
        "rejected: {:?}",
        report.rejected
    );
}

pub(super) fn wave(engine: &HubDistillEngine) -> HubDistillReport {
    engine
        .consolidate_hubs(BANK, &ctx(BANK), &ResolvedRetention::default())
        .unwrap()
}

/// A confirmed discard through the real discard tool.
pub(super) fn discard_item(store: &Arc<clio_store::SqliteStore>, item_id: &str) {
    let as_dyn: Arc<dyn Store> = store.clone();
    discard(&DiscardArgs {
        store: as_dyn.as_ref(),
        ctx: &ctx(BANK),
        item_id,
        reason: "withdrawn by the operator",
        confirm: true,
        timestamp: "2026-06-02T00:00:00Z",
        sink: &DiscardVecSink::default(),
    })
    .unwrap();
}

/// Add a source to the cluster: the item plus the coactivation edge that makes
/// it a candidate neighbour of the hub.
///
/// The item carries the wave clock's own millisecond, so a source added mid-test
/// is current rather than looking like it changed after the reconciliation —
/// the same stamp `seed_hub` gives the sources it seeds.
pub(super) fn add_source(store: &Arc<clio_store::SqliteStore>, id: &str) {
    store.create_memory_item(&source_item(id), "agent").unwrap();
    store
        .reinforce_assoc_edge(BANK, HUB, id, 0, 0.15)
        .expect("reinforce");
}

/// The `BANK`/`HUB` source item, stamped in the wave clock's own millisecond.
///
/// `semantic()` writes `updated_at` as a 2026 ISO date, and the engines here are
/// pinned at millis 0, so an item left on that stamp reads as changed long after
/// any reconciliation built from it. Harmless while that was read-time only; not
/// harmless now that a wave records the lapse on the record.
fn source_item(id: &str) -> clio_types::MemoryItem {
    let mut item = semantic(id, BANK, &format!("neighbor {id}"));
    item.updated_at = WAVE_CLOCK_STAMP.to_owned();
    item
}

pub(super) fn record_of(
    store: &Arc<clio_store::SqliteStore>,
    unit: &str,
) -> clio_store::DistilledUnitRecord {
    store.get_distilled_unit(BANK, unit).unwrap().unwrap()
}

#[test]
fn adding_a_neighbor_writes_a_revision_and_retains_the_prior() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    add_source(&store, "n4");

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2);
    assert!(record.source_ids.contains(&"n4".to_owned()));
    assert_eq!(record.stale_reason, None);

    // The current revision carries the new neighbour's text.
    let current = store
        .get_memory_item(BANK, &record.current_revision_item_id())
        .unwrap()
        .unwrap();
    assert_eq!(current.id, REV2);
    let gist = current.gist.as_deref().unwrap_or_default().to_owned();
    assert!(gist.contains("neighbor n4"), "text: {gist}");
    assert_eq!(current.source_ref.as_deref(), Some(HUB_DISTILL_MARKER));

    // The prior revision is closed on both axes and retained with its text.
    let prior = store.get_memory_item(BANK, UNIT).unwrap().unwrap();
    assert!(prior.valid_until.is_some(), "valid axis closed");
    assert!(prior.tx_until.is_some(), "transaction axis closed");
    assert!(prior.gist.is_some());
    assert!(
        !prior
            .gist
            .as_ref()
            .is_some_and(|g| g.contains("neighbor n4"))
    );

    // The record and the snapshot agree after the revision.
    let snapshot = current.snapshot.unwrap();
    assert_eq!(
        snapshot["neighbor_ids"],
        serde_json::json!(record.source_ids)
    );
    assert_eq!(
        snapshot["neighbor_count"],
        serde_json::json!(record.source_ids.len())
    );
    assert_eq!(snapshot["hub_id"], serde_json::json!(HUB));
    assert_eq!(snapshot.get("stale_reason"), None);
    assert_eq!(current.tx_until, None, "the revision is current");
}

#[test]
fn changing_the_hubs_gist_writes_a_revision() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    let mut hub_item = store.get_memory_item(BANK, HUB).unwrap().unwrap();
    hub_item.gist = Some("hub core rewritten".to_owned());
    store.update_memory_item(&hub_item, "agent").unwrap();

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2);
    let current = store
        .get_memory_item(BANK, &record.current_revision_item_id())
        .unwrap()
        .unwrap();
    assert!(
        current.gist.unwrap().contains("hub core rewritten"),
        "the rebuilt text carries the hub's current gist"
    );
}

#[test]
fn a_wave_over_an_unchanged_cluster_writes_nothing() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    let before_record = record_of(&store, UNIT);
    let before_gist = store.get_gist(BANK, UNIT).unwrap();

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_unchanged,
        vec![HUB.to_owned()],
        "{report:?}"
    );
    assert!(report.revised.is_empty(), "revised: {:?}", report.revised);
    assert_eq!(record_of(&store, UNIT), before_record);
    assert_eq!(store.get_gist(BANK, UNIT).unwrap(), before_gist);
    assert!(
        store.get_memory_item(BANK, REV2).unwrap().is_none(),
        "no revision row may exist for an unchanged cluster"
    );
}

#[test]
fn a_rejected_revision_keeps_the_prior_and_marks_the_unit_stale() {
    let (store, mut normal_engine) = seeded();
    first_wave(&mut normal_engine);
    add_source(&store, "n4");
    // A stricter policy refuses the re-derived revision.
    let mut strict_engine = engine(&store, 1.0, 2);
    strict_engine.set_prune_after_distill(false);

    let report = wave(&strict_engine);
    assert_eq!(report.reconcile_rejected, vec![HUB.to_owned()]);
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 1, "the refused revision replaced nothing");
    assert_eq!(record.stale_reason.as_deref(), Some("revision_rejected"));
    assert!(record.stale_at.is_some());
    assert!(
        record.stale_detail.is_some(),
        "the rejection detail is recorded"
    );
    // The prior revision stays current in the store.
    let prior = store.get_memory_item(BANK, UNIT).unwrap().unwrap();
    assert_eq!(prior.tx_until, None);
    assert!(prior.gist.is_some());
    // The operator can see why: the current item's snapshot names the reason.
    let snapshot = store.get_snapshot(BANK, UNIT).unwrap().unwrap();
    assert_eq!(snapshot["stale_reason"], "revision_rejected");

    // A later wave under the normal policy rebuilds and clears stale.
    let report = wave(&normal_engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2);
    assert_eq!(record.stale_reason, None);
}

#[test]
fn a_wave_after_a_discard_rebuilds_without_the_removed_source() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    discard_item(&store, "n1");

    // The cascade marked the unit stale for source removal.
    let after = record_of(&store, UNIT);
    assert_eq!(after.stale_reason.as_deref(), Some("source_removed"));
    assert!(!after.source_ids.contains(&"n1".to_owned()));

    // The next wave rebuilds the text from the remaining claim set.
    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()]);
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2);
    assert_eq!(
        record.stale_reason, None,
        "stale cleared on a successful revision"
    );
    assert!(!record.source_ids.contains(&"n1".to_owned()));
    let current = store
        .get_memory_item(BANK, &record.current_revision_item_id())
        .unwrap()
        .unwrap();
    let gist = current.gist.as_deref().unwrap_or_default().to_owned();
    assert!(
        !gist.contains("neighbor n1"),
        "the removed source's text must be gone: {gist}"
    );
    // The prior revision is retained with the old text (audit trail).
    let prior = store.get_memory_item(BANK, UNIT).unwrap().unwrap();
    assert!(prior.gist.clone().unwrap().contains("neighbor n1"));
    assert!(prior.tx_until.is_some());

    // The rebuild is stable: a further wave writes nothing (the recomputed
    // selection excludes the discarded source, so no resurrection loop).
    let report = wave(&engine);
    assert_eq!(report.reconcile_unchanged, vec![HUB.to_owned()]);
    assert_eq!(record_of(&store, UNIT).revision, 2);
}

#[test]
fn a_stale_unit_is_reconciled_even_when_its_hub_fell_below_the_limit() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    // Prune the cluster's coactivation edges: the hub's degree collapses and
    // it is no longer an eligible hub.
    for edge in store.list_assoc_edges(BANK, Some(HUB)).unwrap() {
        if edge.assoc_kind == AssocKind::Coactivation {
            store.delete_assoc_edge(BANK, &edge.id).unwrap();
        }
    }
    let hubs = engine.eligible_hubs(BANK).unwrap();
    assert!(hubs.is_empty(), "pruned edges kept a hub: {hubs:?}");
    discard_item(&store, "n1");

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");
    let record = record_of(&store, UNIT);
    assert_eq!(record.stale_reason, None);
    assert!(
        !record.source_ids.contains(&"n1".to_owned()),
        "the rebuilt claim set excludes the removed source"
    );
}
