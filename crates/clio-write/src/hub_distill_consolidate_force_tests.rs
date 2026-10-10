// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `consolidate` forced-wave tests: that
//! `force` performs the existing MemTree dirty-path refresh in the same wave
//! that reconciles the bank's lapsed consolidated units, that the unforced
//! wave keeps its own reconcile rule, and that a bank without units is an
//! honest no-op.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - The forced-wave composition over `consolidate_tool`
//! - That a refused revision during a forced wave is reported, not swallowed
//!
//! ## Does not own
//!
//! - The reconcile decision itself (`hub_distill_reconcile_tests`)
//! - The wave report model (`hub_distill_model`)
//!
//! ## Boundary
//!
//! Changes to how `force` composes the wave should normally require changes
//! here. Changes to what reconciliation does with a unit should not.

use super::hub_distill_tests::{WAVE_CLOCK_STAMP, ctx, engine_at, seed_hub, semantic, store};
use crate::maintenance::LeafIndex;
use crate::memtree::{AttachLeaf, MemForest};
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::consolidate_tool;
use clio_config::retention::ResolvedRetention;
use clio_store::{Store, unit_freshness};
use clio_types::ToolResult;
use std::sync::{Arc, Mutex};

/// The unit one seeded three-neighbour hub distils to.
const UNIT: &str = "hub-distill:bank:hub";

/// A maintenance engine over a tree with one attached leaf, so a forced
/// dirty-all has structural work to do.
fn maint_with_one_leaf() -> Arc<MemTreeMaint> {
    let forest = Arc::new(Mutex::new(MemForest::with_fanout(2)));
    let leaves = Arc::new(LeafIndex::new());
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    forest
        .lock()
        .unwrap()
        .tree_mut("bank")
        .attach_leaf(AttachLeaf {
            item_id: "leaf-1".into(),
            bank_id: "bank".into(),
            interval_start: "1".into(),
            interval_end: "1".into(),
            gist: Some("leaf core".into()),
            task_id: None,
        })
        .unwrap();
    maint
}

/// Run the first wave (no force) and assert it distilled the seeded hub.
fn first_wave(store: &Arc<clio_store::SqliteStore>) -> Arc<MemTreeMaint> {
    let maint = maint_with_one_leaf();
    let engine = Arc::new(engine_at(store, 0.0, 2, 0));
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        false,
        true,
    ) else {
        panic!("the first wave must succeed");
    };
    assert!(data.hubs.contains("distilled=1"), "{}", data.hubs);
    maint
}

/// Edit one claimed source after the reconciliation: its gist moves and its
/// stamp moves past the wave's own clock, so the unit is lapsed until a later
/// revision re-derives it.
fn edit_source(store: &Arc<clio_store::SqliteStore>) {
    let mut edited = store.get_memory_item("bank", "n1").unwrap().unwrap();
    edited.gist = Some("neighbor n1 edited".into());
    edited.updated_at = "1000".into();
    store.update_memory_item(&edited, "agent").unwrap();
}

#[test]
fn force_performs_the_dirty_path_refresh_and_reconciles_the_lapsed_unit() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let maint = first_wave(&store);

    edit_source(&store);
    let record = store.get_distilled_unit("bank", UNIT).unwrap().unwrap();
    let before = unit_freshness(&*store, "bank", &record).unwrap();
    assert_eq!(
        before.stale,
        Some(true),
        "the moved source stamp must lapse the unit"
    );

    // The engine's clock is past the edited source's stamp, so the revision
    // this wave writes leaves the unit current rather than lapsed again.
    let engine = Arc::new(engine_at(&store, 0.0, 2, 2000));
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        true,
        true,
    ) else {
        panic!("the forced wave must succeed");
    };
    assert!(
        !data.refreshed.is_empty(),
        "force must refresh the dirty tree: {data:?}"
    );
    assert!(data.hubs.contains("revised=1"), "{}", data.hubs);

    let record = store.get_distilled_unit("bank", UNIT).unwrap().unwrap();
    assert_eq!(record.revision, 2, "the unit was re-derived");
    assert_eq!(record.stale_reason, None);
    let after = unit_freshness(&*store, "bank", &record).unwrap();
    assert_eq!(
        after.stale,
        Some(false),
        "one forced call leaves the unit current"
    );
}

#[test]
fn without_force_the_normal_wave_rule_still_reconciles_a_lapsed_unit() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let maint = first_wave(&store);
    edit_source(&store);

    // The wave's normal reconcile rule reaches this unit on the unforced wave
    // as well; `force` is not the only path to a rebuild, it is the path that
    // also guarantees the dirty-path refresh above.
    let engine = Arc::new(engine_at(&store, 0.0, 2, 2000));
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        false,
        true,
    ) else {
        panic!("the wave must succeed");
    };
    assert!(data.hubs.contains("revised=1"), "{}", data.hubs);
    let record = store.get_distilled_unit("bank", UNIT).unwrap().unwrap();
    assert_eq!(record.revision, 2);
    assert_eq!(record.stale_reason, None);
}

#[test]
fn a_forced_wave_reports_a_reconcile_rejection_and_rewrites_nothing() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let maint = first_wave(&store);

    // A fourth neighbour joins the cluster, so the unit's claim set is no
    // longer settled and the next wave re-derives it.
    let mut joined = semantic("n4", "bank", "neighbor n4");
    joined.updated_at = WAVE_CLOCK_STAMP.into();
    store.create_memory_item(&joined, "agent").unwrap();
    store
        .reinforce_assoc_edge("bank", "hub", "n4", 0, 0.15)
        .expect("reinforce");

    // The engine admits at theta 1.0, above the new claim's 0.15 coactivation
    // edge, so the re-derived revision fails its gate. A forced wave reports
    // the refusal in its own summary instead of claiming success for the unit.
    let mut engine = Arc::new(engine_at(&store, 1.0, 2, 0));
    Arc::get_mut(&mut engine)
        .expect("the engine is not shared yet")
        .set_prune_after_distill(false);
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        true,
        true,
    ) else {
        panic!("the forced wave must report the rejection, not fail the call");
    };
    assert!(
        data.hubs.contains("revised_rejected=1"),
        "the rejection must be reported: {}",
        data.hubs
    );
    assert!(
        data.hubs.contains("stale_revision_rejected=1"),
        "the refusal marks the unit stale by reason: {}",
        data.hubs
    );
    let record = store.get_distilled_unit("bank", UNIT).unwrap().unwrap();
    assert_eq!(record.revision, 1, "the refused revision replaced nothing");
    assert_eq!(record.stale_reason.as_deref(), Some("revision_rejected"));
    assert!(
        record.stale_detail.is_some(),
        "the rejection detail is recorded"
    );
}

#[test]
fn force_on_a_bank_without_consolidated_units_is_an_honest_no_op() {
    let store = store();
    // One edge is below the degree limit of two, so nothing is eligible and no
    // unit exists; the wave must report that rather than invent work.
    seed_hub(&store, "bank", "hub", &["n1"], 0.15);
    let maint = maint_with_one_leaf();
    let engine = Arc::new(engine_at(&store, 0.0, 2, 0));
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        true,
        true,
    ) else {
        panic!("the forced wave must succeed");
    };
    assert_eq!(data.status, "completed");
    assert!(data.hubs.contains("distilled=0"), "{}", data.hubs);
    assert!(data.hubs.contains("stale=0"), "{}", data.hubs);
    assert!(
        store.get_distilled_unit("bank", UNIT).unwrap().is_none(),
        "no unit may be written"
    );
}
