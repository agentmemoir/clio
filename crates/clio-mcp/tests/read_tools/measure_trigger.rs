// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sweeper-side maintenance-trigger
//! measurements: one fired wave, the firing tick, and the declined ticks, each
//! checked against one generous ceiling.
//!
//! ## Owns
//! - The trigger ceilings (`TriggerBounds`)
//! - The trigger measurements (`run`): the fired wave's refresh and budgeted
//!   hub wave, the firing tick, and the declined ticks
//! - The one-shot timing variant for work that cannot be warmed up without
//!   being consumed (`time_once`, `report_once`): timed cold, and the
//!   `MEASURE` line says `samples=1`
//!
//! ## Does not own
//! - The fixture contents (`fixture`)
//! - The shared timing method, the pressure ceilings, and the report lines
//!   (`measure`)
//!
//! ## Boundary
//!
//! Changes to the trigger ceilings should normally require changes here.
//! Changes to the timing method or the pressure ceilings should not.
//!
//! Keep this module focused on the trigger measurements.

use crate::fixture::{Fixture, MAIN_BANK};
use crate::measure::{SAMPLES, Stats, assert_bound, report, time};
use clio_mcp::runtime::McpState;
use clio_types::{Actor, Bank, OpContext};
use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

/// Generous trigger ceilings, in milliseconds, one per measurement.
pub struct TriggerBounds {
    /// One sweeper-side trigger tick (`evaluate_maintenance_trigger`) over
    /// the fixture's banks: the bank list, three pressure reads per bank, and
    /// the decision.
    pub trigger_tick: f64,
    /// One fired maintenance wave: `refresh_bank` plus the budgeted hub wave.
    pub trigger_wave: f64,
}

impl TriggerBounds {
    /// SQLite in-memory ceilings, set from a clean 50-sample run: the declined
    /// trigger tick sits at the 50 ms floor, and the fired wave gets room
    /// above its measured ~1.3 ms for a single cold sample on a shared runner.
    #[must_use]
    pub fn sqlite() -> Self {
        Self {
            trigger_tick: 50.0,
            trigger_wave: 500.0,
        }
    }

    /// Postgres loopback ceilings, set from a clean 50-sample run: the
    /// declined trigger tick rides the `index_drain` step at its measured
    /// ~5.6 ms p95, and the fired wave gets room above its measured ~12 ms
    /// for a single cold sample on a shared runner.
    #[must_use]
    pub fn postgres() -> Self {
        Self {
            trigger_tick: 100.0,
            trigger_wave: 1000.0,
        }
    }
}

/// Time `op` once, cold, and return its duration in milliseconds. A fired
/// wave's fresh work cannot be warmed up without being consumed, so a one-shot
/// measurement takes a single cold sample instead of a p95.
fn time_once<F: FnOnce() -> u64>(op: F) -> f64 {
    let start = Instant::now();
    black_box(op());
    start.elapsed().as_secs_f64() * 1_000.0
}

/// A one-sample `Stats`: a one-shot measurement has one number, so the shared
/// ceiling assert sees the same value at every percentile.
fn once(ms: f64) -> Stats {
    Stats {
        min: ms,
        median: ms,
        p95: ms,
        max: ms,
    }
}

/// Print one single-sample `MEASURE` line and assert the generous ceiling.
fn report_once(backend: &str, fx: &Fixture, component: &str, ms: f64, bound_ms: f64) {
    eprintln!(
        "MEASURE backend={backend} banks={} items={} samples=1 \
         component={component} single_ms={ms:.3} bound_ms={bound_ms:.3} \
         machine=idle-required",
        fx.banks.len(),
        fx.item_count,
    );
    assert_bound(backend, fx, component, &once(ms), bound_ms);
}

/// Wait until no maintenance wave is scheduled or running, so a detached
/// fired wave never contends with the next timed samples. Bounded: a wave
/// that never drains fails the test instead of hanging it.
fn wait_for_wave_quiescence(state: &McpState) {
    for _ in 0..30_000 {
        if state.maint.waves_in_flight() == 0 {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "a fired maintenance wave never drained: waves_in_flight stayed \
         above zero for the whole 30 s wait, so later samples would time \
         against a wave that is still writing"
    );
}

/// The trigger's fired-wave body, as `fire_wave` runs it: the structural
/// refresh, then the hub wave under the caller's hub budget. Returns the
/// number of cleared forest nodes plus distilled items, so `black_box` has
/// something real to hold.
fn fired_wave_body(state: &McpState, budget: usize) -> u64 {
    let cleared = state.maint.refresh_bank(MAIN_BANK).len();
    let hub = state.hub.as_ref().expect("fixture hub engine");
    let ctx = OpContext::new(Bank::new(MAIN_BANK), Actor::System);
    let retention = state.resolved_retention(MAIN_BANK);
    let wave = hub
        .consolidate_hubs_budgeted(MAIN_BANK, &ctx, &retention, budget)
        .expect("budgeted hub wave");
    u64::try_from(cleared + wave.report.distilled.len()).unwrap_or(u64::MAX)
}

/// Time the sweeper-side trigger tick and one fired wave over the fixture.
///
/// The wave is measured the way the trigger fires it — `refresh_bank` and
/// then `consolidate_hubs_budgeted` under the shipped default hub budget —
/// once with real work in front of it and then as 50 steady-state samples
/// where a wave finds nothing new to distill. The tick is measured once as a
/// firing tick (the fixture's stale unit meets the threshold and no cooldown
/// is recorded yet) and then as 50 declined ticks, the steady state between
/// waves. Every fired wave's thread is drained before the next measurement.
pub fn run(backend: &str, fx: &Fixture, bounds: &TriggerBounds) {
    let state: &McpState = &fx.state;
    let budget =
        usize::try_from(state.env.maintenance_triggers.hub_budget_per_wave).unwrap_or(usize::MAX);

    // One fired wave with real work in front of it, as two cold one-shot
    // samples plus a derived total: a wave's fresh work cannot be warmed up
    // without being consumed, so these cannot be sampled.
    let refresh_ms =
        time_once(|| u64::try_from(state.maint.refresh_bank(MAIN_BANK).len()).unwrap_or(u64::MAX));
    let hub_wave_ms = time_once(|| {
        let hub = state.hub.as_ref().expect("fixture hub engine");
        let ctx = OpContext::new(Bank::new(MAIN_BANK), Actor::System);
        let retention = state.resolved_retention(MAIN_BANK);
        let wave = hub
            .consolidate_hubs_budgeted(MAIN_BANK, &ctx, &retention, budget)
            .expect("budgeted hub wave");
        u64::try_from(wave.report.distilled.len()).unwrap_or(u64::MAX)
    });
    report_once(
        backend,
        fx,
        "trigger_wave_refresh",
        refresh_ms,
        bounds.trigger_wave,
    );
    report_once(
        backend,
        fx,
        "trigger_wave_hub_budgeted",
        hub_wave_ms,
        bounds.trigger_wave,
    );
    eprintln!(
        "MEASURE backend={backend} banks={} items={} samples=1 \
         component=trigger_wave_total derivation=refresh+hub_budgeted \
         total_ms={:.3} hub_budget={budget} machine=idle-required",
        fx.banks.len(),
        fx.item_count,
        refresh_ms + hub_wave_ms,
    );

    // The recurring wave: the same budgeted call once its fresh work is gone,
    // which is what every later fired wave costs when it distills nothing new.
    let repeat = time(|| fired_wave_body(state, budget));
    report(
        backend,
        fx,
        "trigger_wave_repeat",
        &repeat,
        bounds.trigger_wave,
    );

    // The firing tick: the fixture's stale unit still meets the threshold and
    // no cooldown is recorded yet, so this first evaluation fires a wave on
    // the main bank and declines on the empty extra banks. The wave runs on
    // its own thread, so drain it before sampling the declined ticks.
    let fire_ms = time_once(|| {
        state.evaluate_maintenance_trigger(Instant::now());
        u64::try_from(state.maint.waves_in_flight()).unwrap_or(u64::MAX)
    });
    wait_for_wave_quiescence(state);
    report_once(
        backend,
        fx,
        "trigger_tick_fire",
        fire_ms,
        bounds.trigger_tick,
    );

    // The declined ticks: the steady state between waves. Each one still
    // reads the bank list and all three pressure sources for every watched
    // bank, so this is the per-cycle cost the trigger cadence pays.
    let tick = time(|| {
        state.evaluate_maintenance_trigger(Instant::now());
        0
    });
    report(
        backend,
        fx,
        "trigger_tick_declined",
        &tick,
        bounds.trigger_tick,
    );

    // Per-bank and cadence derivations: the fixture's three banks stand in
    // for a deployment's bank count, and the trigger's own interval constant
    // is the budget the tick cost has to fit inside.
    let bank_count =
        f64::from(u32::try_from(fx.banks.len()).expect("fixture bank count fits in u32"));
    let interval_ms =
        clio_mcp::maintenance_trigger::MAINTENANCE_TRIGGER_INTERVAL.as_secs_f64() * 1_000.0;
    eprintln!(
        "MEASURE backend={backend} banks={} items={} samples={SAMPLES} \
         component=trigger_tick_per_bank derivation=declined_p95/banks \
         per_bank_ms={:.3} interval_ms={interval_ms:.0} machine=idle-required",
        fx.banks.len(),
        fx.item_count,
        tick.p95 / bank_count,
    );
    eprintln!(
        "MEASURE backend={backend} banks={} items={} samples={SAMPLES} \
         component=trigger_tick_cadence derivation=interval/per_bank \
         banks_within_interval={:.0} interval_ms={interval_ms:.0} machine=idle-required",
        fx.banks.len(),
        fx.item_count,
        interval_ms / (tick.p95 / bank_count),
    );
}
