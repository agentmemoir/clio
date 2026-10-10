// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the maintenance-trigger knob
//! group in the ranking environment: defaults, patch application, and the
//! rejection of bad values and unknown fields.
//!
//! ## Owns
//! - Tests for `MaintenanceTriggerKnobs` defaults and serde behavior
//! - Tests for the patch surface accepting the group
//!
//! ## Does not own
//! - The knob group itself (see `ranking.rs`)
//! - Tests for the other ranking-env groups (see `ranking_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to the maintenance-trigger knobs' shape, defaults, or semantics
//! should normally require a change here. Changes to other ranking-env groups
//! should not require changes here.
//!
//! Keep this module focused on the maintenance-trigger knob group.

use super::*;

#[test]
fn maintenance_trigger_knobs_have_defaults() {
    let knobs = MaintenanceTriggerKnobs::default();
    assert_eq!(knobs.dirty_ancestor_threshold, 1);
    assert_eq!(knobs.eligible_hub_threshold, 1);
    assert_eq!(knobs.stale_unit_threshold, 1);
    assert_eq!(knobs.cooldown_seconds, 300);
    assert_eq!(knobs.max_waves_per_hour, 12);
    assert_eq!(knobs.hub_budget_per_wave, 16);
    assert_eq!(
        RankingEnv::system_defaults().maintenance_triggers,
        knobs,
        "system defaults carry the knob group"
    );

    // An empty group document fills every default, so a patch that omits the
    // fields keeps the documented values.
    let filled: MaintenanceTriggerKnobs = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(filled, knobs);
}

#[test]
fn preview_patch_applies_maintenance_triggers() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            maintenance_triggers: Some(MaintenanceTriggerKnobs {
                dirty_ancestor_threshold: 5,
                eligible_hub_threshold: 0,
                stale_unit_threshold: 3,
                cooldown_seconds: 60,
                max_waves_per_hour: 2,
                hub_budget_per_wave: 8,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("valid maintenance patch");
    assert_eq!(next.maintenance_triggers.dirty_ancestor_threshold, 5);
    assert_eq!(next.maintenance_triggers.hub_budget_per_wave, 8);
    assert_eq!(
        base.maintenance_triggers,
        MaintenanceTriggerKnobs::default(),
        "base must not be mutated"
    );
}

/// Thresholds are inclusive counts, and a threshold of `0` disables its one
/// condition, so the config layer stores `0` without changing its meaning.
#[test]
fn preview_patch_stores_a_zero_threshold_as_disabled() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            maintenance_triggers: Some(MaintenanceTriggerKnobs {
                stale_unit_threshold: 0,
                ..MaintenanceTriggerKnobs::default()
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("zero threshold applies");
    assert_eq!(next.maintenance_triggers.stale_unit_threshold, 0);
}

/// The value range is the unsigned type range: a signed, fractional, or
/// non-numeric value is rejected instead of clamped or dropped.
#[test]
fn maintenance_trigger_knobs_reject_out_of_range_values() {
    for bad in [
        serde_json::json!({"dirty_ancestor_threshold": -1}),
        serde_json::json!({"dirty_ancestor_threshold": 1.5}),
        serde_json::json!({"dirty_ancestor_threshold": "many"}),
    ] {
        assert!(
            serde_json::from_value::<MaintenanceTriggerKnobs>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

/// The patch rejects unknown fields at both levels: a typo'd key must fail,
/// never be silently dropped.
#[test]
fn maintenance_trigger_patch_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<RankingEnvPatch>(serde_json::json!({"maintenance_triggerss": {}}))
            .is_err(),
        "typo'd group key is rejected"
    );
    assert!(
        serde_json::from_value::<RankingEnvPatch>(serde_json::json!(
            {"maintenance_triggers": {"hub_budget": 8}}
        ))
        .is_err(),
        "unknown field inside the group is rejected"
    );
}

/// The two rate knobs reject `0` at patch time: a wave rate of `0` silently
/// switches maintenance off, and a hub budget of `0` still spends a wave while
/// distilling nothing. A rejected patch leaves the stored group untouched.
#[test]
fn preview_patch_rejects_a_zero_rate_knob_and_keeps_the_stored_group() {
    for knob in ["max_waves_per_hour", "hub_budget_per_wave"] {
        let mut group = MaintenanceTriggerKnobs::default();
        match knob {
            "max_waves_per_hour" => group.max_waves_per_hour = 0,
            _ => group.hub_budget_per_wave = 0,
        }
        let base = RankingEnv::system_defaults();
        let err = preview_patch(
            &base,
            &RankingEnvPatch {
                maintenance_triggers: Some(group),
                ..RankingEnvPatch::default()
            },
        )
        .expect_err("zero rate knob is rejected");
        assert_eq!(err.code, ErrorCode::OutOfRange, "{knob}");
        assert!(
            err.message.contains(knob),
            "{knob} is named in the error: {}",
            err.message
        );
        assert_eq!(
            base.maintenance_triggers,
            MaintenanceTriggerKnobs::default(),
            "{knob}: a rejected patch must not mutate the stored group"
        );
    }
}

/// The other four knobs keep `0`: each threshold disables just its own
/// condition, and a cooldown of `0` means no cooldown.
#[test]
fn preview_patch_accepts_zero_for_the_thresholds_and_the_cooldown() {
    let zeroed = MaintenanceTriggerKnobs {
        dirty_ancestor_threshold: 0,
        eligible_hub_threshold: 0,
        stale_unit_threshold: 0,
        cooldown_seconds: 0,
        max_waves_per_hour: 12,
        hub_budget_per_wave: 16,
    };
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            maintenance_triggers: Some(zeroed),
            ..RankingEnvPatch::default()
        },
    )
    .expect("zero thresholds and zero cooldown apply");
    assert_eq!(next.maintenance_triggers, zeroed);
}

/// An in-range patch still succeeds after the two rejections.
#[test]
fn preview_patch_accepts_the_smallest_usable_rate_knobs() {
    let group = MaintenanceTriggerKnobs {
        max_waves_per_hour: 1,
        hub_budget_per_wave: 1,
        ..MaintenanceTriggerKnobs::default()
    };
    let next = preview_patch(
        &RankingEnv::system_defaults(),
        &RankingEnvPatch {
            maintenance_triggers: Some(group),
            ..RankingEnvPatch::default()
        },
    )
    .expect("rate knobs of 1 apply");
    assert_eq!(next.maintenance_triggers, group);
}

/// A patch whose maintenance-trigger group omits every field fills the
/// group's defaults.
#[test]
fn stored_env_without_maintenance_triggers_fills_defaults() {
    let patch: RankingEnvPatch =
        serde_json::from_value(serde_json::json!({"maintenance_triggers": {}})).unwrap();
    let next = preview_patch(&RankingEnv::system_defaults(), &patch).unwrap();
    assert_eq!(
        next.maintenance_triggers,
        MaintenanceTriggerKnobs::default()
    );
}
