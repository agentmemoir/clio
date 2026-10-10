// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distill crash tests: a unit write
//! that fails mid-wave must fail the wave, and the next wave must converge to
//! exactly one complete unit.
//!
//! ## Owns
//! - Test-only coverage of the crash-then-second-wave convergence property
//! - Test-only proof that the seam faults one write once, and bank isolation
//!
//! ## Does not own
//! - The faulting double itself
//! - The wave envelope or the claim lifecycle
//!
//! ## Boundary
//!
//! Changes to what a crashed wave must leave behind should normally require
//! changes here. Changes to the wave or the seam shape should not require
//! changes here.
//!
//! Keep this module focused on crash convergence.
//!
//! Test-only. Never compiled into production builds.

use crate::HubDistillEngine;
use crate::hub_distill::hub_distill_tests::{ctx, policy, seed_hub, semantic, store};
use crate::hub_distill_test_support::CrashSeam;
use clio_config::ranking::RankingEnv;
use clio_config::retention::ResolvedRetention;
use clio_store::{DistilledUnitRecord, DistilledUnitStore, Store};
use clio_types::ErrorCode;
use std::sync::Arc;

/// An engine over the armed seam, pinned at the fixtures' own clock, so decay
/// is a function of the fixture stamps and not of when the suite runs.
fn engine_over(seam: &Arc<CrashSeam>) -> HubDistillEngine {
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 0.0;
    HubDistillEngine::with_clock(Arc::clone(seam) as Arc<dyn Store>, env, policy(2), 0)
}

#[test]
fn a_crashed_unit_write_leaves_an_incomplete_claim_the_next_wave_finishes() {
    let inner = store();
    seed_hub(&inner, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let seam = CrashSeam::new(Arc::clone(&inner) as Arc<dyn Store>);
    seam.arm();
    let engine = engine_over(&seam);
    let retention = ResolvedRetention::default();

    let failed = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .expect_err("the armed seam fails the unit write");
    assert_eq!(failed.code, ErrorCode::Internal, "{failed:?}");
    assert_eq!(seam.fired(), 1, "the fault must have fired");
    let claim = seam
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the crashed wave left its claim behind");
    assert!(!claim.is_complete(), "a crashed wave completes nothing");
    assert!(
        seam.get_distilled_unit("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_none(),
        "the refused write left no unit behind"
    );

    let second = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .unwrap();
    assert_eq!(
        second.report.distilled,
        vec!["hub-distill:bank:hub".to_owned()],
        "{second:?}"
    );
    assert_eq!(seam.fired(), 1, "the second wave ran disarmed");
    let units = seam.list_distilled_units("bank").unwrap();
    assert_eq!(units.len(), 1, "exactly one unit: {units:?}");
    assert!(
        seam.get_distill_intent("bank", "hub")
            .unwrap()
            .is_some_and(|intent| intent.is_complete()),
        "the second wave finished the claim"
    );
}

#[test]
fn the_seam_fails_one_unit_write_then_passes() {
    let seam = CrashSeam::new(Arc::clone(&store()) as Arc<dyn Store>);
    let mut item = semantic("unit1", "bank", "unit one");
    item.source_ref = Some("hub_distilled".into());
    let record = DistilledUnitRecord::new(
        "bank",
        "unit1",
        "hub",
        ["n1".to_owned()],
        "hub_distilled",
        "0",
    );
    seam.arm();
    let failed = seam
        .commit_distilled_unit(&item, &record, "agent")
        .expect_err("the armed seam refuses its one write");
    assert_eq!(failed.code, ErrorCode::Internal, "{failed:?}");
    assert_eq!(seam.fired(), 1, "the fault fired");
    seam.commit_distilled_unit(&item, &record, "agent").unwrap();
    assert_eq!(seam.fired(), 1, "one-shot: no second refusal");
    assert!(
        seam.get_distilled_unit("bank", "unit1").unwrap().is_some(),
        "the write after the fault lands"
    );
}

#[test]
fn a_crashed_claim_stamp_leaves_the_unit_landed_the_next_wave_finishes() {
    let inner = store();
    seed_hub(&inner, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let seam = CrashSeam::new(Arc::clone(&inner) as Arc<dyn Store>);
    seam.arm_claim_completion();
    // Pruning stays off so the second wave still finds the hub eligible and
    // re-enters the existing-unit branch that finishes the claim. With pruning
    // on, the crashed wave's prune removes the cluster edges and the second
    // wave reconciles the unit in the reconcile pass instead; that routing is
    // the pruned-hub test below.
    let mut engine = engine_over(&seam);
    engine.set_prune_after_distill(false);
    let retention = ResolvedRetention::default();

    let failed = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .expect_err("the armed seam fails the claim completion");
    assert_eq!(failed.code, ErrorCode::Internal, "{failed:?}");
    assert_eq!(seam.claim_fired(), 1, "the claim fault must have fired");
    assert_eq!(seam.fired(), 0, "the unit write itself passed");
    let claim = seam
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the crashed wave left its claim behind");
    assert!(!claim.is_complete(), "the crashed wave completed nothing");
    assert!(
        seam.get_distilled_unit("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_some(),
        "the unit landed before the claim stamp"
    );

    let second = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .unwrap();
    assert!(second.report.distilled.is_empty(), "{second:?}");
    assert_eq!(seam.claim_fired(), 1, "the second wave ran disarmed");
    let units = seam.list_distilled_units("bank").unwrap();
    assert_eq!(units.len(), 1, "exactly one unit: {units:?}");
    let claim = seam
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the claim survives the crash window");
    assert!(claim.is_complete(), "the second wave finished the claim");
    assert_eq!(
        claim.revision_item_id, "hub-distill:bank:hub",
        "the claim names the landed unit's revision"
    );
}

#[test]
fn a_pruned_hub_crashed_claim_is_finished_by_the_reconcile_pass() {
    let inner = store();
    seed_hub(&inner, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let seam = CrashSeam::new(Arc::clone(&inner) as Arc<dyn Store>);
    seam.arm_claim_completion();
    // Pruning stays on, the engine's default: the crashed wave prunes the
    // hub's cluster edges after the unit lands, so the next wave finds no
    // eligible hub and must finish the claim in the reconcile pass instead.
    let engine = engine_over(&seam);
    let retention = ResolvedRetention::default();

    let failed = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .expect_err("the armed seam fails the claim completion");
    assert_eq!(failed.code, ErrorCode::Internal, "{failed:?}");
    assert_eq!(seam.claim_fired(), 1, "the claim fault must have fired");
    assert_eq!(seam.fired(), 0, "the unit write itself passed");
    let claim = seam
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the crashed wave left its claim behind");
    assert!(!claim.is_complete(), "the crashed wave completed nothing");
    assert!(
        seam.get_distilled_unit("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_some(),
        "the unit landed before the claim stamp"
    );

    let second = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, usize::MAX)
        .unwrap();
    assert!(second.report.eligible.is_empty(), "{second:?}");
    assert!(
        second.report.reconcile_unchanged == vec!["hub".to_owned()],
        "the second wave reconciled the landed unit: {second:?}"
    );
    assert!(second.report.distilled.is_empty(), "{second:?}");
    assert_eq!(seam.claim_fired(), 1, "the second wave ran disarmed");
    let units = seam.list_distilled_units("bank").unwrap();
    assert_eq!(units.len(), 1, "exactly one unit: {units:?}");
    let claim = seam
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the claim survives the crash window");
    assert!(claim.is_complete(), "the reconcile pass finished the claim");
    assert_eq!(
        claim.revision_item_id, "hub-distill:bank:hub",
        "the claim names the landed unit's revision"
    );
}

#[test]
fn a_crashed_wave_in_one_bank_leaves_another_bank_alone() {
    let inner = store();
    seed_hub(&inner, "bank-a", "hub-a", &["n1", "n2", "n3"], 0.15);
    seed_hub(&inner, "bank-b", "hub-b", &["m1", "m2", "m3"], 0.15);
    let seam = CrashSeam::new(Arc::clone(&inner) as Arc<dyn Store>);
    seam.arm();
    let engine = engine_over(&seam);
    let retention = ResolvedRetention::default();

    let failed = engine
        .consolidate_hubs_budgeted("bank-a", &ctx("bank-a"), &retention, usize::MAX)
        .expect_err("the armed seam fails bank-a's unit write");
    assert_eq!(failed.code, ErrorCode::Internal, "{failed:?}");
    assert_eq!(seam.fired(), 1, "the fault fired");

    let other = engine
        .consolidate_hubs_budgeted("bank-b", &ctx("bank-b"), &retention, usize::MAX)
        .unwrap();
    assert_eq!(
        other.report.distilled,
        vec!["hub-distill:bank-b:hub-b".to_owned()],
        "{other:?}"
    );
    assert!(
        seam.get_distilled_unit("bank-a", "hub-distill:bank-a:hub-a")
            .unwrap()
            .is_none(),
        "bank-a kept no unit from its crashed wave"
    );
    let units = seam.list_distilled_units("bank-b").unwrap();
    assert_eq!(units.len(), 1, "bank-b holds its own unit: {units:?}");
    assert!(
        seam.get_distill_intent("bank-b", "hub-b")
            .unwrap()
            .is_some_and(|intent| intent.is_complete()),
        "bank-b's claim is complete"
    );
}
