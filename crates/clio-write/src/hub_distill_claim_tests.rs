// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distill claim tests: the wave claims
//! a hub before working and completes the claim after, and the next wave
//! reconciles, resumes, or isolates that claim.
//!
//! ## Owns
//! - Test-only coverage of the claim-before-work ordering property
//! - Test-only proof of reconcile, resume, and bank isolation for claims
//!
//! ## Does not own
//! - The claim lifecycle itself
//! - The wave envelope or the crash-fault behavior
//!
//! ## Boundary
//!
//! Changes to what a claim must order or leave behind should normally require
//! changes here. Changes to the wave or the seam shape should not require
//! changes here.
//!
//! Keep this module focused on claim ordering.
//!
//! Test-only. Never compiled into production builds.

use crate::hub_distill::hub_distill_tests::{ctx, engine, seed_hub, semantic, store};
use clio_config::retention::ResolvedRetention;
use clio_store::{DistillIntent, DistilledUnitStore, Store};

fn unit_id(bank: &str, hub: &str) -> String {
    format!("hub-distill:{bank}:{hub}")
}

#[test]
fn wave_claims_hub_before_working_and_completes_with_revision() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.distilled, vec![unit_id("bank", "hub")]);
    let intent = store
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("the wave claims the hub");
    assert!(intent.is_complete(), "the claim completes with the wave");
    assert_eq!(intent.unit_item_id, unit_id("bank", "hub"));
    assert_eq!(
        intent.revision_item_id,
        unit_id("bank", "hub"),
        "the first revision names the unit id"
    );
    assert_eq!(
        intent.claimed_at, "1970-01-01T00:00:00Z",
        "the claim is stamped in the wave clock"
    );
}

#[test]
fn second_wave_for_complete_hub_reconciles_without_writes() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let mut engine = engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    let first = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(first.distilled, vec![unit_id("bank", "hub")]);
    let before = store
        .get_distilled_unit("bank", &unit_id("bank", "hub"))
        .unwrap()
        .expect("unit after the first wave");

    let second = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(second.reconcile_unchanged, vec!["hub".to_owned()]);
    assert!(second.distilled.is_empty(), "{second:?}");

    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(
        outcome.status, "reconciled_current",
        "a complete hub takes the existing reconcile outcome: {outcome:?}"
    );
    assert!(
        outcome.reason.is_none(),
        "a complete claim is done work, not a resume: {outcome:?}"
    );
    let after = store
        .get_distilled_unit("bank", &unit_id("bank", "hub"))
        .unwrap()
        .expect("unit after the second wave");
    assert_eq!(before, after, "the no-op reconcile writes nothing");
    assert_eq!(
        store.list_distilled_units("bank").unwrap().len(),
        1,
        "still exactly one unit for the hub"
    );
}

#[test]
fn interrupted_wave_finishes_on_next_wave_with_one_complete_unit() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    // The hand-written stamp matches the wave clock on purpose: the wave must
    // notice the resume from its own claim write, not from a stamp mismatch.
    store
        .write_distill_intent(
            "bank",
            &DistillIntent::new("bank", "hub", unit_id("bank", "hub"), "agent", "0"),
        )
        .unwrap();
    let before = store
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("hand-written claim");
    assert!(
        !before.is_complete(),
        "an interrupted claim stays incomplete"
    );

    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "distilled", "{outcome:?}");
    assert!(
        outcome
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("resum")),
        "the outcome notes the resume: {outcome:?}"
    );
    assert_eq!(
        store.list_distilled_units("bank").unwrap().len(),
        1,
        "exactly one complete unit exists"
    );
    let after = store
        .get_distill_intent("bank", "hub")
        .unwrap()
        .expect("claim after the wave");
    assert!(after.is_complete(), "the resumed wave completes the claim");
    assert_eq!(after.revision_item_id, unit_id("bank", "hub"));

    let again = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(again.status, "reconciled_current", "{again:?}");
    assert_eq!(
        store.list_distilled_units("bank").unwrap().len(),
        1,
        "the resumed hub converges on one unit"
    );
}

#[test]
fn completed_claim_without_unit_takes_the_normal_missing_item_path() {
    let store = store();
    store
        .create_memory_item(&semantic("lonely", "bank", "lonely core"), "agent")
        .unwrap();
    store
        .write_distill_intent(
            "bank",
            &DistillIntent::new("bank", "lonely", unit_id("bank", "lonely"), "agent", "0"),
        )
        .unwrap();
    assert!(
        store
            .complete_distill_intent("bank", "lonely", &unit_id("bank", "lonely"))
            .unwrap(),
        "the claim starts complete with no unit behind it"
    );
    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "lonely", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "no_neighbors");
    assert!(outcome.reason.is_some(), "{outcome:?}");
    assert!(
        store
            .get_memory_item("bank", &unit_id("bank", "lonely"))
            .unwrap()
            .is_none(),
        "the wave resurrects nothing"
    );
    assert!(
        store.list_distilled_units("bank").unwrap().is_empty(),
        "no unit is duplicated behind the complete claim"
    );
}

#[test]
fn bank_a_claim_is_invisible_to_bank_b_wave() {
    let store = store();
    seed_hub(&store, "a", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let first_a = engine
        .consolidate_hubs("a", &ctx("a"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(first_a.distilled, vec![unit_id("a", "hub")]);

    // Bank B holds an incomplete claim for the same hub id string, with no
    // items behind it: item ids are bank-scoped nowhere, so the same hub item
    // cannot exist in both banks, and the claim row is where the isolation
    // has to hold.
    store
        .write_distill_intent(
            "b",
            &DistillIntent::new("b", "hub", unit_id("b", "hub"), "agent", "0"),
        )
        .unwrap();
    let report_b = engine
        .consolidate_hubs("b", &ctx("b"), &ResolvedRetention::default())
        .unwrap();
    assert!(report_b.eligible.is_empty(), "{report_b:?}");

    let claim_a = store
        .get_distill_intent("a", "hub")
        .unwrap()
        .expect("bank A's claim");
    assert!(claim_a.is_complete(), "{claim_a:?}");
    assert_eq!(claim_a.unit_item_id, unit_id("a", "hub"));
    let claim_b = store
        .get_distill_intent("b", "hub")
        .unwrap()
        .expect("bank B's claim");
    assert!(
        !claim_b.is_complete(),
        "bank B's wave never saw bank A's work"
    );
    assert_eq!(
        engine
            .distill_hub("b", "hub", &ctx("b"), ResolvedRetention::default())
            .expect_err("no hub item in bank B")
            .code,
        clio_types::ErrorCode::ItemNotFound
    );
}
