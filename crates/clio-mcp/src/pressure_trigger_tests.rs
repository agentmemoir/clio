// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `trigger` block's unit tests: the
//! never-evaluated marker, the declined decision's rendering, the elapsed
//! wave-time renderings, the passed-cooldown rendering, and the block's merge
//! into `maintenance_status` straight from the trigger's own record. It also
//! owns the pressure block's last-wave snapshot rendering: the wave tally a
//! productive wave records, rendered from the engine's own snapshot.
//!
//! ## Owns
//! - The last-wave snapshot rendering test (`stale_units`)
//! - The declined-pressure-read rendering test (`pressure: None` renders null)
//! - Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The trigger block itself (`pressure`)
//! - The trigger's decision and record (`maintenance_trigger`)
//!
//! ## Boundary
//!
//! Tests here break when the trigger block's shape changes; they never change
//! production behavior.
//!
//! Keep this module focused on the trigger block's honest rendering contract.

use super::pressure_tests::{BANK, commit_unit, state, state_for_bank};
use super::trigger_block;
use crate::maintenance_trigger::{DeclineReason, MaintenanceTriggerStatus, PressureCounts};
use crate::read_tools::dispatch;
use clio_config::ranking::MaintenanceTriggerKnobs;
use clio_config::retention::ResolvedRetention;
use clio_store::AssocPolicy;
use clio_types::{Actor, Bank, OpContext};
use clio_write::HubDistillEngine;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The cooldown the synthetic statuses carry, so the elapsed renderings are
/// hand-computable.
const COOLDOWN: Duration = Duration::from_secs(60);

/// One evaluated pressure read, nonzero so a fabricated zero cannot pass.
const PRESSURE: PressureCounts = PressureCounts {
    dirty_ancestors: 3,
    eligible_hubs: 1,
    stale_units: 2,
};

/// A synthetic snapshot with the fixed cooldown; wave times are parameters.
fn status(
    fired: bool,
    decline: Option<DeclineReason>,
    last_wave: Option<Instant>,
    waves_last_hour: usize,
) -> MaintenanceTriggerStatus {
    MaintenanceTriggerStatus {
        bank: BANK.to_owned(),
        pressure: Some(PRESSURE),
        fired,
        last_wave,
        next_eligible: last_wave.map(|wave| wave + COOLDOWN),
        waves_last_hour,
        last_decline: decline,
    }
}

/// The render instant and a wave instant `wave_ago` seconds before it, from
/// one clock read, so the elapsed times are exact and no test reads the wall
/// clock for its assertions.
fn render_instants(wave_ago: u64) -> (Instant, Option<Instant>) {
    let now = Instant::now();
    let wave = now
        .checked_sub(Duration::from_secs(wave_ago))
        .expect("test base instant predates the program start");
    (now, Some(wave))
}

#[test]
fn a_never_evaluated_bank_renders_an_explicit_marker() {
    let block = trigger_block(None, Instant::now());
    assert_eq!(
        block,
        json!({
            "evaluated": false,
            "unavailable": "trigger has not evaluated this bank",
        }),
        "{block}"
    );
}

#[test]
fn a_declined_evaluation_carries_the_counts_and_the_reason() {
    let block = trigger_block(
        Some(status(false, Some(DeclineReason::NoPressure), None, 0)),
        Instant::now(),
    );
    assert_eq!(
        block,
        json!({
            "evaluated": true,
            "fired": false,
            "last_decline": "no pressure",
            "waves_last_hour": 0,
            "pressure": {
                "dirty_ancestors": 3,
                "eligible_hubs": 1,
                "stale_units": 2,
            },
            "last_wave_secs_ago": Value::Null,
            "next_eligible_in_secs": Value::Null,
        }),
        "{block}"
    );
}

#[test]
fn a_fired_evaluation_reports_elapsed_wave_times() {
    // One wave ten seconds ago: its elapsed time and the remaining cooldown
    // are the hand-computed difference against the injected render instant.
    let (now, wave) = render_instants(10);
    let block = trigger_block(Some(status(true, None, wave, 1)), now);
    assert_eq!(block["fired"], json!(true), "{block}");
    assert_eq!(block["last_decline"], Value::Null, "{block}");
    assert_eq!(block["waves_last_hour"], json!(1), "{block}");
    assert_eq!(block["last_wave_secs_ago"], json!(10), "{block}");
    assert_eq!(block["next_eligible_in_secs"], json!(60 - 10), "{block}");
}

#[test]
fn a_passed_cooldown_renders_zero_seconds_until_eligible() {
    // A wave two cooldowns ago: the elapsed time is honest and the already
    // passed next-eligible time renders zero, never a negative count.
    let (now, wave) = render_instants(2 * 60);
    let block = trigger_block(Some(status(false, None, wave, 1)), now);
    assert_eq!(block["last_wave_secs_ago"], json!(2 * 60), "{block}");
    assert_eq!(block["next_eligible_in_secs"], json!(0), "{block}");
}

#[test]
fn the_trigger_block_reports_the_next_eligible_time_past_the_hour() {
    // A wave fifty-nine minutes ago under a two-hour cooldown: the elapsed
    // time is honest and the remaining wait runs past the hourly window,
    // because the record keeps the wave instant the hourly count prunes.
    let cooldown = Duration::from_secs(7200);
    let (now, wave) = render_instants(3540);
    let record = MaintenanceTriggerStatus {
        next_eligible: wave.map(|w| w + cooldown),
        ..status(true, None, wave, 1)
    };
    let block = trigger_block(Some(record), now);
    assert_eq!(block["last_wave_secs_ago"], json!(3540), "{block}");
    assert_eq!(block["next_eligible_in_secs"], json!(3660), "{block}");

    // A bank with no wave yet says so explicitly instead of rendering a zero
    // that would read as a wave that just ran.
    let never = trigger_block(
        Some(status(false, Some(DeclineReason::NoPressure), None, 0)),
        now,
    );
    assert_eq!(never["last_wave_secs_ago"], Value::Null, "{never}");
    assert_eq!(never["next_eligible_in_secs"], Value::Null, "{never}");
    let unevaluated = trigger_block(None, now);
    assert_eq!(
        unevaluated["unavailable"],
        json!("trigger has not evaluated this bank"),
        "{unevaluated}"
    );
}

#[test]
fn maintenance_status_carries_the_trigger_from_the_trigger_record() {
    let s = state_for_bank(BANK);
    let out = dispatch(&s, "maintenance_status", &json!({"bank": BANK})).expect("dispatch");
    // The fresh state was never evaluated: the block states that explicitly.
    assert_eq!(
        out["trigger"],
        json!({
            "evaluated": false,
            "unavailable": "trigger has not evaluated this bank",
        })
    );
    // One declined evaluation in the shared record: the block renders that
    // record, so it can never disagree with the decision that was made.
    let decision = s.maintenance_trigger.evaluate(
        BANK,
        &MaintenanceTriggerKnobs::default(),
        true,
        Some(PressureCounts::default()),
        0,
        0,
        Instant::now(),
    );
    assert_eq!(
        decision,
        crate::maintenance_trigger::TriggerDecision::Decline(DeclineReason::NoPressure),
        "{decision:?}"
    );
    let out = dispatch(&s, "maintenance_status", &json!({"bank": BANK})).expect("dispatch");
    assert_eq!(out["trigger"]["evaluated"], json!(true));
    assert_eq!(out["trigger"]["fired"], json!(false));
    assert_eq!(out["trigger"]["last_decline"], json!("no pressure"));
}

/// An evaluation whose pressure read was declined (lock held) records no
/// counts: the block renders that record's `null` pressure verbatim, never a
/// fabricated zero, and never the unevaluated marker.
#[test]
fn an_evaluation_without_a_pressure_read_renders_null_pressure() {
    let record = MaintenanceTriggerStatus {
        pressure: None,
        ..status(false, Some(DeclineReason::NoPressure), None, 0)
    };
    let block = trigger_block(Some(record), Instant::now());
    assert_eq!(block["evaluated"], json!(true), "{block}");
    assert_eq!(block["pressure"], Value::Null, "{block}");
}

/// A productive wave records its stale tally against the engine's own clock,
/// and the pressure block renders that snapshot verbatim: a wave that found a
/// stale unit can never read as a bank with no wave.
#[test]
fn a_productive_wave_renders_the_last_wave_snapshot() {
    let s = state();
    // One stored-stale unit, so the wave's reconcile pass visits a unit and
    // the wave records a tally; the clock is pinned, so the stamp is exact.
    commit_unit(&s, "unit-stale", true);
    let engine = Arc::new(HubDistillEngine::with_clock(
        s.store.clone(),
        s.env.clone(),
        AssocPolicy::default(),
        0,
    ));
    let ctx = OpContext::new(Bank::new(BANK), Actor::parse("agent").expect("actor"));
    engine
        .consolidate_hubs(BANK, &ctx, &ResolvedRetention::default())
        .expect("wave");
    let mut scoped = (*s).clone();
    scoped.hub = Some(engine);

    let out = super::stale_units(&scoped, BANK);
    assert_eq!(out["count"], json!(1), "{out}");
    assert_eq!(out["source"], json!("stored_state"), "{out}");
    assert_eq!(
        out["last_wave"],
        json!({
            "at_millis": 0,
            "tally": {
                "source_removed": 0,
                "source_changed": 1,
                "revision_rejected": 0,
                "unknown": 0,
            },
        }),
        "{out}"
    );
}
