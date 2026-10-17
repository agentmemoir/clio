// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the automatic maintenance trigger: the
//! sweeper-side evaluation that decides whether a bank's maintenance pressure
//! justifies a wave, and the per-bank decision record the status surfaces read.
//!
//! ## Owns
//! - `MaintenanceTriggerState`: the per-bank evaluation record and its
//!   non-blocking snapshot (`MaintenanceTriggerStatus`)
//! - `McpState::evaluate_maintenance_trigger`: read the active banks' three
//!   pressure counts without waiting for a lock a write needs, decide, record
//!   the outcome, and fire eligible waves
//! - The pure wave decision: inclusive thresholds, cooldown, hourly cap, and
//!   exactly one decline reason per evaluated tick
//!
//! ## Does not own
//! - The sweeper loop and its cadence (`runtime_index`)
//! - The wave's work itself (`clio-write`; the manual `consolidate` path's
//!   execution pattern is reused, never re-implemented)
//! - The pressure counts' sources (`pressure`), and the hub engine's own
//!   per-bank in-flight count (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to when an automatic wave may run, or how a tick's outcome is
//! recorded, should normally require changes here. Changes to how a wave does
//! its work, or how pressure is reported, should not require changes here.
//!
//! Keep this module focused on the trigger's decision and its recorded state.

use crate::runtime::McpState;
use clio_config::ranking::MaintenanceTriggerKnobs;
use clio_config::retention::ResolvedRetention;
use clio_types::{Actor, Bank, OpContext};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "maintenance_trigger_activity.rs"]
mod maintenance_trigger_activity;

/// Trigger evaluation cadence on the sweeper thread: ten index sweeps. The
/// per-tick duty stays far below the drain's, so the drain's cadence and the
/// post-write nudge are untouched.
pub const MAINTENANCE_TRIGGER_INTERVAL: Duration = Duration::from_millis(5_000);

/// Window the hourly cap counts waves over.
const HOURLY_WINDOW: Duration = Duration::from_secs(3600);

/// One bank's triggering pressure counts, as an evaluation read them.
///
/// Only the three counts that can fire a wave; the pressure block's
/// reported-only counts never trigger one and are not read here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PressureCounts {
    /// Dirty ancestor nodes in the bank's tree.
    pub dirty_ancestors: u64,
    /// Hubs the eligibility scan finds.
    pub eligible_hubs: u64,
    /// Stored distilled units carrying a stale reason.
    pub stale_units: u64,
}

/// Why one evaluated tick declined to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclineReason {
    /// No enabled condition met its threshold.
    NoPressure,
    /// A wave fired too recently for the configured cooldown.
    Cooldown,
    /// The configured hourly wave cap is already used up.
    Budget,
    /// Another maintenance wave is scheduled or running.
    Contention,
    /// The bank's retention profile switched the trigger off.
    TriggerDisabled,
}

impl DeclineReason {
    /// The stable status-surface label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoPressure => "no pressure",
            Self::Cooldown => "cooldown",
            Self::Budget => "budget",
            Self::Contention => "contention",
            Self::TriggerDisabled => "trigger disabled",
        }
    }
}

/// One evaluated tick's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerDecision {
    /// Fire one maintenance wave for the bank.
    Fire,
    /// Record one decline reason and run nothing.
    Decline(DeclineReason),
}

/// Everything the decision reads. A plain value with `now` as a parameter, so
/// tests inject instants instead of waiting on the wall clock.
struct DecisionInput<'a> {
    knobs: &'a MaintenanceTriggerKnobs,
    enabled: bool,
    pressure: PressureCounts,
    waves_in_flight: usize,
    /// Hub-distillation waves running for this bank. Counted separately from
    /// the wave slot, which a wave releases before it distils hubs.
    hub_distill_in_flight: usize,
    /// Wave instants inside the hourly window, for the per-hour cap.
    wave_times: &'a [Instant],
    /// The last fired wave's instant, kept outside the hourly window so a
    /// cooldown longer than one hour still has a wave to measure from.
    last_wave: Option<Instant>,
    now: Instant,
}

/// The pure wave decision.
///
/// Reason order (first blocker wins): trigger disabled, no pressure, cooldown,
/// contention, budget. Thresholds are inclusive; a threshold of zero disables
/// its one condition, so a count can never meet it.
fn decide(input: &DecisionInput) -> TriggerDecision {
    if !input.enabled {
        return TriggerDecision::Decline(DeclineReason::TriggerDisabled);
    }
    let knobs = input.knobs;
    let pressure = input.pressure;
    let has_pressure = (knobs.dirty_ancestor_threshold > 0
        && pressure.dirty_ancestors >= knobs.dirty_ancestor_threshold)
        || (knobs.eligible_hub_threshold > 0
            && pressure.eligible_hubs >= knobs.eligible_hub_threshold)
        || (knobs.stale_unit_threshold > 0 && pressure.stale_units >= knobs.stale_unit_threshold);
    if !has_pressure {
        return TriggerDecision::Decline(DeclineReason::NoPressure);
    }
    if let Some(last) = input.last_wave {
        let elapsed = input.now.checked_duration_since(last);
        if elapsed.is_none_or(|since| since < Duration::from_secs(knobs.cooldown_seconds)) {
            // A recorded instant after `now` (impossible under the monotonic
            // clock in production) stays inside the cooldown.
            return TriggerDecision::Decline(DeclineReason::Cooldown);
        }
    }
    if input.waves_in_flight > 0 || input.hub_distill_in_flight > 0 {
        return TriggerDecision::Decline(DeclineReason::Contention);
    }
    if waves_in_window(input.wave_times, input.now) >= knobs.max_waves_per_hour {
        return TriggerDecision::Decline(DeclineReason::Budget);
    }
    TriggerDecision::Fire
}

/// Waves recorded strictly inside the hour ending at `now`.
fn waves_in_window(wave_times: &[Instant], now: Instant) -> u64 {
    let cutoff = now.checked_sub(HOURLY_WINDOW);
    wave_times
        .iter()
        .filter(|t| cutoff.is_none_or(|cutoff| **t > cutoff))
        .count() as u64
}

/// Per-bank record the evaluation writes and the status surfaces read.
#[derive(Debug, Default)]
struct BankRecord {
    /// The last evaluation's pressure read (`None` before the first one, and
    /// on a tick whose read was declined as contended).
    pressure: Option<PressureCounts>,
    /// Whether the last evaluation fired a wave.
    fired: bool,
    /// Wave instants, pruned to [`HOURLY_WINDOW`] on every evaluation. Drives
    /// the per-hour cap and the one-hour count only.
    wave_times: Vec<Instant>,
    /// The last fired wave's instant. Never pruned: the cooldown reads it, so
    /// pruning it to the hour window would make a cooldown longer than one
    /// hour behave as one hour.
    last_wave: Option<Instant>,
    /// The cooldown that governed the last evaluation, so a snapshot can name
    /// the next eligible time without re-reading configuration.
    cooldown_seconds: u64,
    /// Why the last evaluation declined (`None` after a fired wave).
    last_decline: Option<DeclineReason>,
}

/// What one bank's trigger record looks like from outside. A plain value, so
/// a reader never touches the trigger's lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceTriggerStatus {
    /// The evaluated bank.
    pub bank: String,
    /// The last evaluation's pressure read; `None` before the first one, and
    /// on a tick whose read was declined because a lock was held.
    pub pressure: Option<PressureCounts>,
    /// Whether the last evaluation fired a wave.
    pub fired: bool,
    /// The last automatic wave's start instant. Kept for the whole process
    /// lifetime, so a cooldown longer than the hourly window still has a wave
    /// to measure from and the block still names the next eligible time.
    pub last_wave: Option<Instant>,
    /// When the cooldown from [`Self::last_wave`] ends.
    pub next_eligible: Option<Instant>,
    /// Waves recorded inside the last hour, as of the last evaluation.
    pub waves_last_hour: usize,
    /// Why the last evaluation declined; `None` after a fired wave.
    pub last_decline: Option<DeclineReason>,
}

/// The trigger's shared record, keyed by bank, plus the active-bank registry.
///
/// Behind an `Arc` on [`McpState`], so every clone of the state reads one
/// record. Locks are held only inside short non-async methods.
#[derive(Debug, Default)]
pub struct MaintenanceTriggerState {
    /// Banks this runtime served, noted at the per-call resolution point. A
    /// tick evaluates these plus the default (and shared) bank instead of
    /// listing every bank in the store on every tick.
    active: Mutex<HashSet<String>>,
    banks: Mutex<HashMap<String, BankRecord>>,
}

impl MaintenanceTriggerState {
    /// Decide for one bank from its recorded state, then record the outcome.
    ///
    /// `pressure` is `None` when the tick could not read the bank's pressure
    /// without waiting for a lock a write needs. That tick is not decided at
    /// all: a count read as zero would hide the pressure behind a lock it
    /// cannot take, so the tick records contention instead, runs nothing, and
    /// never retries.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn evaluate(
        &self,
        bank: &str,
        knobs: &MaintenanceTriggerKnobs,
        enabled: bool,
        pressure: Option<PressureCounts>,
        waves_in_flight: usize,
        hub_distill_in_flight: usize,
        now: Instant,
    ) -> TriggerDecision {
        let mut banks = self.banks.lock().expect("trigger state mutex poisoned");
        let record = banks.entry(bank.to_owned()).or_default();
        record.pressure = pressure;
        let decision = match pressure {
            Some(pressure) => decide(&DecisionInput {
                knobs,
                enabled,
                pressure,
                waves_in_flight,
                hub_distill_in_flight,
                wave_times: &record.wave_times,
                last_wave: record.last_wave,
                now,
            }),
            None => TriggerDecision::Decline(DeclineReason::Contention),
        };
        match decision {
            TriggerDecision::Fire => {
                record.fired = true;
                record.last_decline = None;
                record.wave_times.push(now);
                record.last_wave = Some(now);
            }
            TriggerDecision::Decline(reason) => {
                record.fired = false;
                record.last_decline = Some(reason);
            }
        }
        record.cooldown_seconds = knobs.cooldown_seconds;
        let cutoff = now.checked_sub(HOURLY_WINDOW);
        record
            .wave_times
            .retain(|t| cutoff.is_none_or(|cutoff| *t > cutoff));
        decision
    }

    /// One bank's record snapshot; `None` for a never-evaluated bank.
    #[must_use]
    pub fn status(&self, bank: &str) -> Option<MaintenanceTriggerStatus> {
        let banks = self.banks.lock().expect("trigger state mutex poisoned");
        let record = banks.get(bank)?;
        let last_wave = record.last_wave;
        Some(MaintenanceTriggerStatus {
            bank: bank.to_owned(),
            pressure: record.pressure,
            fired: record.fired,
            last_wave,
            next_eligible: last_wave
                .map(|wave| wave + Duration::from_secs(record.cooldown_seconds)),
            waves_last_hour: record.wave_times.len(),
            last_decline: record.last_decline,
        })
    }
}

impl McpState {
    /// Evaluate every active bank's maintenance pressure and fire eligible
    /// waves.
    ///
    /// Runs on the sweeper thread at [`MAINTENANCE_TRIGGER_INTERVAL`]; the
    /// caller passes its one clock read so the whole tick shares an instant.
    /// Each evaluated bank gets exactly one recorded outcome. A fired wave
    /// runs on its own thread — never on the sweeper loop and never inside a
    /// write — through the same execution pattern the manual `consolidate`
    /// tool uses.
    pub fn evaluate_maintenance_trigger(&self, now: Instant) {
        let knobs = {
            let runtime = self.config.lock().expect("config runtime mutex poisoned");
            runtime.ranking_env().maintenance_triggers
        };
        // Evaluate only the banks this runtime has resolved, plus the default
        // (and shared) bank. The store's full historical bank list is never
        // read here, so an idle tick issues no unbounded query.
        for bank in self
            .maintenance_trigger
            .tick_banks(&self.default_bank, self.shared_enabled)
        {
            self.evaluate_bank_trigger(&bank, &knobs, now);
        }
    }

    /// Evaluate one bank: read its pressure, decide, record, fire.
    fn evaluate_bank_trigger(&self, bank: &str, knobs: &MaintenanceTriggerKnobs, now: Instant) {
        // None when a read would have to wait for a lock a write needs: the
        // tick records contention and runs nothing for this bank.
        let pressure = pressure_counts(self, bank);
        // The retention profile owns the on/off switch, and the same resolved
        // profile governs the wave's gating if one fires.
        let retention = self.resolved_retention(bank);
        // Both signals, so the second-wave guard covers the hub phase as well
        // as the structural refresh the wave slot covers.
        let hub_distill_in_flight = self
            .hub
            .as_ref()
            .map_or(0, |hub| hub.hub_distill_in_flight(bank));
        let decision = self.maintenance_trigger.evaluate(
            bank,
            knobs,
            retention.maintenance_trigger(),
            pressure,
            self.maint.waves_in_flight(),
            hub_distill_in_flight,
            now,
        );
        if decision == TriggerDecision::Fire {
            fire_wave(self, bank, knobs, retention);
        }
    }
}

/// Read a bank's three triggering pressure counts.
///
/// The same sources the `pressure` block reports: the live dirty-ancestor
/// set, the hub engine's eligibility scan, and the stored stale-unit count.
/// Read-only: no summary is recomputed here, and a failed source reads as no
/// pressure from that condition rather than fabricating one.
///
/// The dirty-ancestor count is read through the maintenance accessor's
/// non-blocking path, so this tick never waits for the forest lock a write
/// takes. `None` means that count could not be read without waiting: a write
/// or a wave holds the lock. It is contention, never zero dirty ancestors,
/// which would hide real pressure behind a held lock.
fn pressure_counts(state: &McpState, bank: &str) -> Option<PressureCounts> {
    let dirty_ancestors = state.maint.try_dirty_ancestor_count(bank)? as u64;
    let eligible_hubs = state
        .hub
        .as_ref()
        .and_then(|hub| hub.eligible_hubs(bank).ok())
        .map_or(0, |hubs| hubs.len() as u64);
    let stale_units = state.store.list_distilled_units(bank).map_or(0, |units| {
        units
            .iter()
            .filter(|unit| unit.stale_reason.is_some())
            .count() as u64
    });
    Some(PressureCounts {
        dirty_ancestors,
        eligible_hubs,
        stale_units,
    })
}

/// Spawn the fired wave off the sweeper loop, in the manual `consolidate`
/// tool's execution pattern: a detached thread runs the structural refresh,
/// then the hub wave under the per-wave hub budget. The wave's own audit rows
/// are the record; nobody is left to report a failure to.
fn fire_wave(
    state: &McpState,
    bank: &str,
    knobs: &MaintenanceTriggerKnobs,
    retention: ResolvedRetention,
) {
    let maint = Arc::clone(&state.maint);
    let hub = state.hub.clone();
    let bank = bank.to_owned();
    // No caller exists, so the wave acts as the process: the system actor.
    let ctx = OpContext::new(Bank::new(&bank), Actor::System);
    let budget = usize::try_from(knobs.hub_budget_per_wave).unwrap_or(usize::MAX);
    // Claim the in-flight slot before spawning, not inside the new thread: a
    // waiter between the spawn and the thread's first instruction would see an
    // idle queue and could run a competing wave. One claim here, one release
    // in the thread once the structural refresh finishes; the hub wave runs
    // after the release, exactly as the manual consolidate path does.
    maint.begin_wave();
    thread::spawn(move || {
        let _ = maint.refresh_bank_inner(&bank);
        maint.end_wave();
        if let Some(hub) = hub {
            let _ = hub.consolidate_hubs_budgeted(&bank, &ctx, &retention, budget);
        }
    });
}

#[cfg(test)]
#[path = "maintenance_trigger_tests.rs"]
mod maintenance_trigger_tests;

#[cfg(test)]
#[path = "maintenance_trigger_more_tests.rs"]
mod maintenance_trigger_more_tests;
