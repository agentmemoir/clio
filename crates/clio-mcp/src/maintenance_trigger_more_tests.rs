// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance trigger's remaining tests:
//! the live configuration patch that must govern the next evaluation, the
//! multi-bank sweep, a cooldown longer than the hourly window, and every way a
//! contended read declines instead of firing.
//!
//! ## Owns
//! - The live trigger-knob patch and the multi-bank sweep assertions
//! - The cooldown, contention, and contended-read tests against a real state
//!
//! ## Does not own
//! - The trigger's own decision table (`maintenance_trigger_tests`)
//! - The trigger block's rendering (`pressure_trigger_tests`)
//!
//! ## Boundary
//!
//! Changes to the trigger's liveness and contention contract should normally
//! require changes here. Changes to how a maintenance wave's work is scheduled
//! should not require changes here.
//!
//! Keep this module focused on the trigger's liveness and contention contract.
//!
//! The tests here break when the trigger's live configuration read or its
//! recorded contention handling changes; they never change production behavior.

use super::maintenance_trigger_tests::{
    BANK, base, commit_stale_unit, dirty_pressure, input, item_in, state, wait_for_waves,
};
use super::*;
use clio_config::ranking::{MaintenanceTriggerKnobs, RankingEnvPatch};
use serde_json::{Value, json};
use std::sync::mpsc;
use std::thread;

/// A second bank, so one sweep can carry a pressured bank and a quiet one.
const OTHER_BANK: &str = "trigger-other-bank";

/// Publish the whole maintenance-trigger knob group the way `ranking_env_set`
/// does, from the state the sweeper reads it from.
pub(super) fn set_trigger_knobs(state: &McpState, knobs: MaintenanceTriggerKnobs) {
    let mut rt = state.config.lock().expect("config runtime mutex poisoned");
    let patch = RankingEnvPatch {
        maintenance_triggers: Some(knobs),
        ..RankingEnvPatch::default()
    };
    assert!(
        matches!(
            rt.ranking_env_set(&patch, false),
            clio_config::error::ToolResult::Ok { .. }
        ),
        "ranking_env_set should apply"
    );
}

/// Raise one threshold past the seeded pressure, leaving the rest at default.
fn raised_stale_threshold() -> MaintenanceTriggerKnobs {
    MaintenanceTriggerKnobs {
        stale_unit_threshold: 5,
        ..MaintenanceTriggerKnobs::default()
    }
}

#[test]
fn a_live_knob_patch_governs_the_next_evaluation() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    set_trigger_knobs(&state, raised_stale_threshold());
    state.evaluate_maintenance_trigger(base());
    let held_back = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(
        held_back.last_decline,
        Some(DeclineReason::NoPressure),
        "the patched threshold above the read count governed the decision"
    );
    assert!(
        !held_back.fired,
        "the raised threshold must not fire a wave"
    );
    assert_eq!(state.maint.waves_in_flight(), 0, "no wave ran");

    // The same live patch lowered past the count fires on the very next tick,
    // so the decision reads the live group and never a cached copy.
    set_trigger_knobs(&state, MaintenanceTriggerKnobs::default());
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(1));
    let lowered = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert!(lowered.fired, "the lowered threshold governed the decision");
    assert_eq!(lowered.last_decline, None);
    wait_for_waves(&state);
}

#[test]
fn one_sweep_evaluates_every_active_bank() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    state
        .store
        .create_memory_item(&item_in("quiet-2", OTHER_BANK), "agent")
        .expect("seed second bank");
    state.maintenance_trigger.note_bank(OTHER_BANK);
    state.evaluate_maintenance_trigger(base());

    let pressured = state
        .maintenance_trigger
        .status(BANK)
        .expect("the pressured bank was evaluated");
    let quiet = state
        .maintenance_trigger
        .status(OTHER_BANK)
        .expect("the quiet bank was evaluated in the same sweep");
    assert!(pressured.fired, "the pressured bank fired its wave");
    assert_eq!(pressured.last_decline, None);
    assert_eq!(
        pressured.pressure,
        Some(PressureCounts {
            stale_units: 1,
            ..PressureCounts::default()
        })
    );
    assert!(!quiet.fired, "the quiet bank fired nothing");
    assert_eq!(quiet.last_decline, Some(DeclineReason::NoPressure));
    assert_eq!(quiet.last_wave, None);
    assert_eq!(quiet.waves_last_hour, 0);
    wait_for_waves(&state);
}

#[test]
fn resolving_a_bank_marks_it_active() {
    let state = state();
    let ctx = state
        .resolve_ctx(&json!({"bank": OTHER_BANK}))
        .expect("resolve the bank");
    assert_eq!(ctx.bank.as_str(), OTHER_BANK);
    assert!(
        state
            .maintenance_trigger
            .active_banks()
            .iter()
            .any(|bank| bank == OTHER_BANK),
        "the resolved bank is active"
    );
    state.evaluate_maintenance_trigger(base());
    assert!(
        state.maintenance_trigger.status(OTHER_BANK).is_some(),
        "the resolved bank was swept"
    );
}

#[test]
fn idle_ticks_evaluate_only_the_active_banks() {
    let state = state();
    // Seed many banks in the store that this runtime never resolved. A tick
    // that listed the store's historical banks would evaluate every one.
    for i in 0..32 {
        let bank = format!("idle-bank-{i}");
        state
            .store
            .create_memory_item(&item_in(&format!("idle-{i}"), &bank), "agent")
            .expect("seed idle bank");
    }
    // Two idle ticks with no resolved call between them. The fixture bank is
    // quiet, so the first tick evicts it and the registry is empty; no store
    // bank ever entered it.
    state.evaluate_maintenance_trigger(base());
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(5));
    assert!(
        state.maintenance_trigger.active_banks().is_empty(),
        "a quiet resolved bank leaves the active set and no store bank enters it"
    );
    for i in 0..32 {
        let bank = format!("idle-bank-{i}");
        assert!(
            state.maintenance_trigger.status(&bank).is_none(),
            "the sweep evaluated an idle store bank `{bank}`"
        );
    }
}

#[test]
fn a_cooldown_longer_than_the_hourly_window_still_suppresses() {
    let knobs = MaintenanceTriggerKnobs {
        cooldown_seconds: 7200,
        ..MaintenanceTriggerKnobs::default()
    };
    let start = base();
    let mut inside = input(
        &knobs,
        dirty_pressure(),
        start + Duration::from_secs(3605),
        &[],
    );
    inside.last_wave = Some(start);
    assert_eq!(
        decide(&inside),
        TriggerDecision::Decline(DeclineReason::Cooldown),
        "a wave an hour and a minute old is still inside a two-hour cooldown"
    );
    let mut at_end = input(
        &knobs,
        dirty_pressure(),
        start + Duration::from_secs(7200),
        &[],
    );
    at_end.last_wave = Some(start);
    assert_eq!(decide(&at_end), TriggerDecision::Fire);

    // The record keeps the wave instant the hourly window prunes away, so the
    // second wave still measures its cooldown from the first one.
    let state = state();
    assert_eq!(
        state
            .maintenance_trigger
            .evaluate(BANK, &knobs, true, Some(dirty_pressure()), 0, 0, start,),
        TriggerDecision::Fire
    );
    assert_eq!(
        state.maintenance_trigger.evaluate(
            BANK,
            &knobs,
            true,
            Some(dirty_pressure()),
            0,
            0,
            start + Duration::from_secs(3605),
        ),
        TriggerDecision::Decline(DeclineReason::Cooldown)
    );
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_wave, Some(start));
    assert_eq!(
        status.next_eligible,
        Some(start + Duration::from_secs(7200))
    );
    assert_eq!(status.waves_last_hour, 0, "the wave aged out of the hour");
    assert_eq!(
        state.maintenance_trigger.evaluate(
            BANK,
            &knobs,
            true,
            Some(dirty_pressure()),
            0,
            0,
            start + Duration::from_secs(7200),
        ),
        TriggerDecision::Fire
    );
}

#[test]
fn a_bank_with_no_wave_yet_renders_no_wave_times() {
    let state = state();
    assert_eq!(
        state.maintenance_trigger.evaluate(
            OTHER_BANK,
            &MaintenanceTriggerKnobs::default(),
            true,
            Some(PressureCounts::default()),
            0,
            0,
            base(),
        ),
        TriggerDecision::Decline(DeclineReason::NoPressure)
    );
    let status = state
        .maintenance_trigger
        .status(OTHER_BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_wave, None);
    assert_eq!(status.next_eligible, None);
    let block = crate::pressure::trigger_block(Some(status), base());
    assert_eq!(block["last_wave_secs_ago"], Value::Null, "{block}");
    assert_eq!(block["next_eligible_in_secs"], Value::Null, "{block}");
}

#[test]
fn a_contended_pressure_read_declines_and_records_contention() {
    let state = state();
    let decision = state.maintenance_trigger.evaluate(
        BANK,
        &MaintenanceTriggerKnobs::default(),
        true,
        None,
        0,
        0,
        base(),
    );
    assert_eq!(
        decision,
        TriggerDecision::Decline(DeclineReason::Contention)
    );
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_decline, Some(DeclineReason::Contention));
    assert_eq!(
        status.pressure, None,
        "a contended read is never recorded as a zero count"
    );
    assert!(!status.fired);
    assert_eq!(status.last_wave, None);
    assert_eq!(state.maint.waves_in_flight(), 0, "no wave slot was claimed");
}

#[test]
fn a_held_forest_lock_declines_instead_of_waiting() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    state
        .store
        .create_memory_item(&item_in("quiet-1", BANK), "agent")
        .expect("seed bank");

    // A holder thread keeps the forest lock until this test releases it, so the
    // contended read below is a real lock failure and not a race with a thread
    // that already let go.
    let forest = Arc::clone(&state.forest);
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = thread::spawn(move || {
        let _guard = forest.lock().expect("forest");
        locked_tx.send(()).expect("signal the lock is held");
        release_rx.recv().expect("wait for release");
    });
    locked_rx.recv().expect("holder took the forest lock");

    let start = Instant::now();
    state.evaluate_maintenance_trigger(base());
    let elapsed = start.elapsed();
    release_tx.send(()).expect("release the holder");
    holder.join().expect("join the holder");

    assert!(
        elapsed < MAINTENANCE_TRIGGER_INTERVAL,
        "the sweep returned in {elapsed:?}, well under one trigger interval, \
         so it declined rather than waited for the lock"
    );
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_decline, Some(DeclineReason::Contention));
    assert_eq!(status.pressure, None);
    assert!(!status.fired);
    assert_eq!(state.maint.waves_in_flight(), 0, "no wave ran");
}

#[test]
fn a_wave_distilling_hubs_declines_a_second_start() {
    let knobs = MaintenanceTriggerKnobs::default();
    let now = base();
    let mut hub_wave = input(&knobs, dirty_pressure(), now, &[]);
    hub_wave.hub_distill_in_flight = 1;
    assert_eq!(
        decide(&hub_wave),
        TriggerDecision::Decline(DeclineReason::Contention),
        "a hub wave that already released the wave slot still guards the bank"
    );
    let mut wave = input(&knobs, dirty_pressure(), now, &[]);
    wave.waves_in_flight = 1;
    assert_eq!(
        decide(&wave),
        TriggerDecision::Decline(DeclineReason::Contention),
        "a structural refresh in flight guards the bank too"
    );
}
