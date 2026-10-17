// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance trigger's tests: the pure
//! decision's threshold, cooldown, budget, contention, and switch behavior;
//! and the evaluation's recorded outcomes against a real runtime state.
//!
//! ## Owns
//! - Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The trigger itself (`maintenance_trigger`)
//! - The stores and engines it reads (`clio-store`, `clio-write`)
//!
//! ## Boundary
//!
//! Tests here break when the trigger's decision or recorded shape changes;
//! they never change production behavior.
//!
//! Keep this module focused on the trigger's decision and record contract.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use clio_config::retention::RetentionProfile;
use clio_store::{DistilledUnitRecord, STALE_SOURCE_CHANGED};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use clio_write::HUB_DISTILL_MARKER;
use serde_json::json;
use std::time::Duration;

pub(super) const BANK: &str = "trigger-bank";
const CREATED: &str = "2026-10-01T00:00:00Z";

pub(super) fn state() -> RetentionStoreFixture {
    let state = RetentionStoreFixture::open_with_effective(&McpOpenOptions::default(), &json!({}));
    state.maintenance_trigger.watch_bank(BANK);
    state
}

/// One real instant as the base; every other instant is derived from it, so
/// every assertion is a relative offset and the wall clock is read once.
pub(super) fn base() -> Instant {
    Instant::now()
}

/// A decision input over the default knobs with pressure, no waves, and no
/// contention; tests override the one field their claim needs.
pub(super) fn input<'a>(
    knobs: &'a MaintenanceTriggerKnobs,
    pressure: PressureCounts,
    now: Instant,
    wave_times: &'a [Instant],
) -> DecisionInput<'a> {
    DecisionInput {
        knobs,
        enabled: true,
        pressure,
        waves_in_flight: 0,
        hub_distill_in_flight: 0,
        wave_times,
        last_wave: None,
        now,
    }
}

/// Dirty pressure at the default threshold.
pub(super) fn dirty_pressure() -> PressureCounts {
    PressureCounts {
        dirty_ancestors: 1,
        ..PressureCounts::default()
    }
}

#[test]
fn no_pressure_declines_without_a_wave() {
    let knobs = MaintenanceTriggerKnobs::default();
    let decision = decide(&input(&knobs, PressureCounts::default(), base(), &[]));
    assert_eq!(
        decision,
        TriggerDecision::Decline(DeclineReason::NoPressure)
    );
}

#[test]
fn each_condition_fires_at_its_inclusive_threshold() {
    let knobs = MaintenanceTriggerKnobs::default();
    let now = base();
    let dirty = decide(&input(&knobs, dirty_pressure(), now, &[]));
    let hubs = decide(&input(
        &knobs,
        PressureCounts {
            eligible_hubs: 1,
            ..PressureCounts::default()
        },
        now,
        &[],
    ));
    let stale = decide(&input(
        &knobs,
        PressureCounts {
            stale_units: 1,
            ..PressureCounts::default()
        },
        now,
        &[],
    ));
    assert_eq!(dirty, TriggerDecision::Fire);
    assert_eq!(hubs, TriggerDecision::Fire);
    assert_eq!(stale, TriggerDecision::Fire);
}

#[test]
fn zero_threshold_disables_its_condition() {
    let knobs = MaintenanceTriggerKnobs {
        dirty_ancestor_threshold: 0,
        ..MaintenanceTriggerKnobs::default()
    };
    let decision = decide(&input(&knobs, dirty_pressure(), base(), &[]));
    assert_eq!(
        decision,
        TriggerDecision::Decline(DeclineReason::NoPressure)
    );
}

#[test]
fn a_disabled_threshold_leaves_the_other_conditions_live() {
    let knobs = MaintenanceTriggerKnobs {
        dirty_ancestor_threshold: 0,
        ..MaintenanceTriggerKnobs::default()
    };
    // A large dirty count the disabled condition ignores, plus one eligible
    // hub and one stale unit: each of the other two still fires a wave.
    assert_eq!(
        decide(&input(
            &knobs,
            PressureCounts {
                dirty_ancestors: 9,
                eligible_hubs: 1,
                stale_units: 1,
            },
            base(),
            &[],
        )),
        TriggerDecision::Fire
    );
}

#[test]
fn a_threshold_above_any_count_leaves_the_other_conditions_live() {
    let knobs = MaintenanceTriggerKnobs {
        dirty_ancestor_threshold: u64::MAX,
        ..MaintenanceTriggerKnobs::default()
    };
    assert_eq!(
        decide(&input(&knobs, dirty_pressure(), base(), &[])),
        TriggerDecision::Decline(DeclineReason::NoPressure),
        "a threshold no count can meet declines, never fires"
    );
    assert_eq!(
        decide(&input(
            &knobs,
            PressureCounts {
                stale_units: 1,
                ..PressureCounts::default()
            },
            base(),
            &[],
        )),
        TriggerDecision::Fire,
        "the other condition still fires"
    );
}

#[test]
fn disabled_trigger_declines_before_pressure() {
    let knobs = MaintenanceTriggerKnobs::default();
    let mut input = input(
        &knobs,
        PressureCounts {
            dirty_ancestors: 9,
            ..PressureCounts::default()
        },
        base(),
        &[],
    );
    input.enabled = false;
    assert_eq!(
        decide(&input),
        TriggerDecision::Decline(DeclineReason::TriggerDisabled)
    );
}

#[test]
fn cooldown_blocks_inside_the_window_and_releases_at_its_end() {
    let knobs = MaintenanceTriggerKnobs::default();
    let start = base();
    let waves = [start];
    let mut inside_input = input(
        &knobs,
        dirty_pressure(),
        start + Duration::from_secs(100),
        &waves,
    );
    inside_input.last_wave = Some(start);
    let mut boundary_input = input(
        &knobs,
        dirty_pressure(),
        start + Duration::from_secs(300),
        &waves,
    );
    boundary_input.last_wave = Some(start);
    let inside = decide(&inside_input);
    let boundary = decide(&boundary_input);
    assert_eq!(inside, TriggerDecision::Decline(DeclineReason::Cooldown));
    assert_eq!(boundary, TriggerDecision::Fire);
}

#[test]
fn hourly_cap_declines_at_the_limit() {
    let knobs = MaintenanceTriggerKnobs::default();
    let start = base();
    let at_cap: Vec<Instant> = (0..12).map(|i| start + Duration::from_secs(i)).collect();
    // Past the last wave's cooldown (300s) but inside the hourly window, so
    // the cap is the first blocker.
    let now = start + Duration::from_secs(360);
    assert_eq!(
        decide(&input(&knobs, dirty_pressure(), now, &at_cap)),
        TriggerDecision::Decline(DeclineReason::Budget)
    );
    assert_eq!(
        decide(&input(&knobs, dirty_pressure(), now, &at_cap[..11])),
        TriggerDecision::Fire
    );
}

#[test]
fn a_wave_in_flight_declines_with_contention() {
    let knobs = MaintenanceTriggerKnobs::default();
    let mut input = input(&knobs, dirty_pressure(), base(), &[]);
    input.waves_in_flight = 1;
    assert_eq!(
        decide(&input),
        TriggerDecision::Decline(DeclineReason::Contention)
    );
}

#[test]
fn waves_age_out_of_the_hourly_window() {
    let knobs = MaintenanceTriggerKnobs::default();
    let start = base();
    let old: Vec<Instant> = (0..12).map(|i| start + Duration::from_secs(i)).collect();
    let now = start + HOURLY_WINDOW + Duration::from_secs(1);
    assert_eq!(
        decide(&input(&knobs, dirty_pressure(), now, &old)),
        TriggerDecision::Fire
    );
}

#[test]
fn the_trigger_interval_is_ten_index_sweeps() {
    assert_eq!(
        MAINTENANCE_TRIGGER_INTERVAL.as_millis(),
        10 * crate::index_drain::INDEX_SWEEP_INTERVAL.as_millis()
    );
}

pub(super) fn item(id: &str) -> MemoryItem {
    item_in(id, BANK)
}

/// One item in a named bank, so a test can seed a second bank beside the one
/// [`BANK`] names.
pub(super) fn item_in(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: id.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: Some(format!("turn-{id}")),
        snapshot: None,
        gist: Some("trigger fixture summary".into()),
        valid_from: CREATED.into(),
        valid_until: None,
        tx_from: CREATED.into(),
        tx_until: None,
        created_at: CREATED.into(),
        updated_at: CREATED.into(),
        context: None,
    }
}

/// Commit one distilled unit marked stale, so the stored stale-unit count
/// carries pressure.
pub(super) fn commit_stale_unit(state: &McpState, id: &str) {
    let mut unit = item(id);
    unit.source_ref = Some(HUB_DISTILL_MARKER.to_owned());
    let mut record = DistilledUnitRecord::new(
        BANK,
        id,
        "hub",
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        CREATED,
    );
    record.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
    record.stale_at = Some("2026-10-02T00:00:00Z".to_owned());
    state
        .store
        .commit_distilled_unit(&unit, &record, "agent")
        .expect("commit unit");
}

pub(super) fn wait_for_waves(state: &McpState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.maint.waves_in_flight() > 0 {
        assert!(Instant::now() < deadline, "wave did not finish within 10s");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_quiet_bank_declines_and_records_no_pressure() {
    let state = state();
    state
        .store
        .create_memory_item(&item("quiet-1"), "agent")
        .expect("seed item");
    state.evaluate_maintenance_trigger(base());
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.pressure, Some(PressureCounts::default()));
    assert!(!status.fired);
    assert_eq!(status.last_decline, Some(DeclineReason::NoPressure));
    assert_eq!(status.last_wave, None);
    assert_eq!(status.next_eligible, None);
    assert_eq!(status.waves_last_hour, 0);
    assert_eq!(state.maint.waves_in_flight(), 0, "no wave ran");
}

#[test]
fn stale_pressure_fires_a_wave_and_records_the_fire() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    let start = base();
    state.evaluate_maintenance_trigger(start);
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert!(status.fired, "stale pressure fired a wave");
    assert_eq!(status.last_decline, None);
    assert_eq!(
        status.pressure,
        Some(PressureCounts {
            stale_units: 1,
            ..PressureCounts::default()
        })
    );
    assert_eq!(status.last_wave, Some(start));
    assert_eq!(status.next_eligible, Some(start + Duration::from_secs(300)));
    assert_eq!(status.waves_last_hour, 1);
    wait_for_waves(&state);
}

#[test]
fn a_second_evaluation_inside_the_cooldown_declines() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    let start = base();
    state.evaluate_maintenance_trigger(start);
    state.evaluate_maintenance_trigger(start + Duration::from_secs(60));
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_decline, Some(DeclineReason::Cooldown));
    assert!(!status.fired);
    assert_eq!(status.waves_last_hour, 1);
    wait_for_waves(&state);
}

#[test]
fn a_disabled_profile_declines_with_trigger_disabled() {
    let state = state();
    commit_stale_unit(&state, "stale-1");
    let profile = RetentionProfile {
        maintenance_trigger: false,
        ..RetentionProfile::default()
    };
    state.set_retention(BANK, profile).expect("set profile");
    state.evaluate_maintenance_trigger(base());
    let status = state
        .maintenance_trigger
        .status(BANK)
        .expect("bank evaluated");
    assert_eq!(status.last_decline, Some(DeclineReason::TriggerDisabled));
    assert!(!status.fired);
    assert_eq!(state.maint.waves_in_flight(), 0, "no wave ran");
}

#[test]
fn evaluation_skips_historical_banks_until_the_session_watches_them() {
    let state = state();
    state
        .ops_store
        .register_bank("historical-bank", CREATED)
        .expect("register historical bank");
    state.evaluate_maintenance_trigger(base());
    assert!(
        state
            .maintenance_trigger
            .status("historical-bank")
            .is_none(),
        "historical banks are not polled just because they exist"
    );
    state.maintenance_trigger.watch_bank("active-bank");
    state
        .ops_store
        .register_bank("active-bank", CREATED)
        .expect("register active bank");
    state.evaluate_maintenance_trigger(base() + Duration::from_secs(5));
    assert!(
        state.maintenance_trigger.status("active-bank").is_some(),
        "a session-watched bank is evaluated"
    );
}
