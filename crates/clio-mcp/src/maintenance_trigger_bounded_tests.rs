// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance trigger's bounded active-bank
//! registry tests: a quiet bank is evicted, a pressured bank is retained until
//! it is quiet, a bank resolved during a tick is not lost, and the default and
//! enabled shared banks stay always-evaluated.
//!
//! ## Owns
//! - Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The active-bank selection (`maintenance_trigger_activity`)
//! - The per-bank decision and record (`maintenance_trigger`)
//! - The fixture state (`maintenance_trigger_tests`)
//!
//! ## Boundary
//!
//! Changes to which banks a tick keeps active should normally require changes
//! here. Changes to the per-bank decision should not.
//!
//! Keep this module focused on the registry's bounded-growth contract.

use super::maintenance_trigger_more_tests::set_trigger_knobs;
use super::maintenance_trigger_tests::{BANK, base, commit_stale_unit, state};
use super::*;
use crate::runtime::{McpOpenOptions, SHARED_BANK};
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use clio_config::ranking::MaintenanceTriggerKnobs;
use serde_json::json;

/// Whether `bank` is in the active registry.
fn active_contains(state: &McpState, bank: &str) -> bool {
    state
        .maintenance_trigger
        .active_banks()
        .iter()
        .any(|active| active == bank)
}

#[test]
fn a_quiet_resolved_bank_is_evicted_after_a_tick() {
    let state = state();
    assert!(
        active_contains(&state, BANK),
        "the resolved bank starts active"
    );
    state.evaluate_maintenance_trigger(base());
    assert!(
        !active_contains(&state, BANK),
        "a bank with no pressure leaves the active set after a tick"
    );
    // The last evaluation is still reportable after eviction.
    assert!(
        state.maintenance_trigger.status(BANK).is_some(),
        "the quiet bank's evaluation record is kept"
    );
}

#[test]
fn a_pressured_bank_is_retained_until_quiet() {
    let state = state();
    // A threshold above the seeded count keeps the wave from firing, so the
    // test observes retention alone and never races the wave thread.
    set_trigger_knobs(
        &state,
        MaintenanceTriggerKnobs {
            stale_unit_threshold: 5,
            ..MaintenanceTriggerKnobs::default()
        },
    );
    commit_stale_unit(&state, "stale-1");
    state.evaluate_maintenance_trigger(base());
    assert!(
        active_contains(&state, BANK),
        "a bank with stale pressure stays active after its tick"
    );
    // A second idle tick with no new resolve must not drop it.
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(5));
    assert!(
        active_contains(&state, BANK),
        "pressure keeps the bank active without a new resolve"
    );

    // Clear the pressure the way a wave would, then the next tick evicts it.
    let mut record = state
        .store
        .get_distilled_unit(BANK, "stale-1")
        .expect("read distilled unit")
        .expect("distilled unit is present");
    record.stale_reason = None;
    record.stale_at = None;
    state
        .store
        .update_distilled_unit(&record, "agent")
        .expect("clear the stale reason");
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(10));
    assert!(
        !active_contains(&state, BANK),
        "the bank is evicted once it is quiet"
    );
}

#[test]
fn a_bank_resolved_during_a_tick_is_not_lost() {
    let state = state();
    // Take the registry the way one tick does, then resolve another bank
    // before the tick merges its pressured banks back.
    let claimed = state
        .maintenance_trigger
        .claim_tick_banks(&state.default_bank, state.shared_enabled);
    assert!(
        claimed.contains(&BANK.to_owned()),
        "the tick claimed the bank"
    );
    let mid_tick = "resolved-mid-tick";
    state.maintenance_trigger.note_bank(mid_tick);
    state.maintenance_trigger.retain_pressured(Vec::new());
    assert!(
        active_contains(&state, mid_tick),
        "a bank resolved during the tick survives the pressured-bank merge"
    );
}

#[test]
fn the_default_bank_is_always_evaluated() {
    let state = state();
    // The resolved bank is quiet and leaves the set; the default bank is not
    // tracked in the registry, so it must still be evaluated on every tick.
    state.evaluate_maintenance_trigger(base());
    assert!(!active_contains(&state, BANK));
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(5));
    assert!(
        state
            .maintenance_trigger
            .status(&state.default_bank)
            .is_some(),
        "the default bank is evaluated even with an empty active set"
    );
}

#[test]
fn an_enabled_shared_bank_is_always_evaluated() {
    let state = RetentionStoreFixture::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: true,
            ..McpOpenOptions::default()
        },
        &json!({}),
    );
    state.evaluate_maintenance_trigger(base());
    assert!(
        state.maintenance_trigger.status(SHARED_BANK).is_some(),
        "the enabled shared bank is evaluated without a resolve"
    );
}
