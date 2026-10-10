// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distill wave envelope: running one
//! bank's hub-distillation wave, bounded by a per-wave hub budget the caller
//! supplies, and recording what the budget left behind.
//!
//! ## Owns
//! - `consolidate_hubs`, the unbounded wave entry
//! - `consolidate_hubs_budgeted`, the budgeted wave entry
//! - `BudgetedWave`, the wave result carrying its deferred-hub count
//! - `hub_distill_in_flight`, the per-bank count of waves running right now,
//!   and the guard that releases each wave's claim on every exit path
//!
//! ## Does not own
//! - The per-hub distill pipeline and the eligibility scan (`hub_distill`,
//!   `hub_distill_scan`)
//! - The reconcile pass and the freshness tally (`hub_distill_reconcile`,
//!   `hub_distill_freshness`)
//! - Report and item shapes and the pure builders (`hub_distill_model`)
//!
//! ## Boundary
//!
//! Split out of `hub_distill.rs` to keep that file inside the 450-line limit.
//! Changes to how a wave bounds and orders its work, or to how a running wave
//! is counted, should normally require changes here. Changes to how one hub is
//! distilled or reconciled should not require changes here.
//!
//! Keep this module focused on the wave envelope and its budget.

use super::HubDistillEngine;
use crate::hub_distill_model::HubDistillReport;
use clio_config::retention::ResolvedRetention;
use clio_types::{AmError, OpContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What one budgeted wave produced, and what it left behind.
#[derive(Debug)]
pub struct BudgetedWave {
    /// The wave's own report. `eligible` still names every hub the scan
    /// found; the loop visited only the first
    /// `eligible.len() - hubs_deferred` of them.
    pub report: HubDistillReport,
    /// Eligible hubs the budget did not let this wave visit; a later wave's
    /// scan can still find and visit them.
    pub hubs_deferred: usize,
}

/// One bank hub wave's claim on that bank's in-flight counter.
///
/// Released when the guard drops, so every exit path releases it: the wave's
/// error return, an early return, and a panic. A count released by hand would
/// leak on one of those paths and wedge that bank out of every later wave.
struct HubWaveClaim {
    /// The bank's counter, claimed before the wave starts its first read.
    counter: Arc<AtomicUsize>,
}

impl Drop for HubWaveClaim {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

impl HubDistillEngine {
    /// Claim one hub-distillation wave against its bank's counter.
    ///
    /// Claimed before the wave's first read, not after: a caller that samples
    /// the counter between the claim and the first instruction would otherwise
    /// see an idle bank and could start a competing wave for it. The returned
    /// guard releases the claim on every exit path.
    fn claim_hub_wave(&self, bank: &str) -> HubWaveClaim {
        let counter = self
            .hub_waves_in_flight
            .lock()
            .expect("hub distill wave gauge")
            .entry(bank.to_owned())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .clone();
        counter.fetch_add(1, Ordering::AcqRel);
        HubWaveClaim { counter }
    }

    /// How many hub-distillation waves are running for `bank` right now.
    ///
    /// Per bank, so one bank's wave never reports another bank as busy.
    /// Non-blocking: one short read of the engine's own counter map, which no
    /// write and no wave holds while doing work, then a relaxed load. It never
    /// waits for a wave, never retries, and starts no work. The count covers
    /// the whole wave, from before its first read to after its last write, so
    /// it stays non-zero for the reconcile pass too.
    #[must_use]
    pub fn hub_distill_in_flight(&self, bank: &str) -> usize {
        self.hub_waves_in_flight
            .lock()
            .expect("hub distill wave gauge")
            .get(bank)
            .map_or(0, |counter| counter.load(Ordering::Relaxed))
    }

    /// Run one hub-distillation wave for a bank (does not block leaf writes).
    ///
    /// The wave takes one policy snapshot at its start and reads the
    /// eligibility scan and every neighbor scan from that one value, so a
    /// `set_policy` that lands mid-wave governs the next wave instead of half
    /// of this one.
    ///
    /// Every distilled item is gated under `retention`, the bank's resolved
    /// retention profile.
    ///
    /// Unbounded: the hub loop visits every eligible hub the scan found. A
    /// caller with a per-wave hub budget should call
    /// [`Self::consolidate_hubs_budgeted`] instead.
    ///
    /// # Errors
    /// Storage failures; admission rejects are recorded, not returned.
    pub fn consolidate_hubs(
        &self,
        bank: &str,
        ctx: &OpContext,
        retention: &ResolvedRetention,
    ) -> Result<HubDistillReport, AmError> {
        Ok(self
            .consolidate_hubs_budgeted(bank, ctx, retention, usize::MAX)?
            .report)
    }

    /// [`Self::consolidate_hubs`] under `hub_budget`, the largest number of
    /// eligible hubs this wave's hub loop may visit (`usize::MAX` for no
    /// cap). The caller supplies the budget; the engine never reads
    /// configuration itself.
    ///
    /// The loop visits the eligibility scan's first
    /// `min(eligible, hub_budget)` hubs in scan order and stops there, so a
    /// wave never runs past its budget. A wave that stops at the budget
    /// records what it left behind: [`BudgetedWave::hubs_deferred`] carries
    /// the count, and the wave's `hub_distill_complete` audit row writes it
    /// as `hubs_deferred`, so the deferral outlives the process.
    ///
    /// The budget bounds the hub loop only. The reconcile pass and the
    /// freshness tally are bounded by the units the bank already has, not by
    /// the budget, and run as before.
    ///
    /// # Errors
    /// Storage failures; admission rejects are recorded, not returned.
    pub fn consolidate_hubs_budgeted(
        &self,
        bank: &str,
        ctx: &OpContext,
        retention: &ResolvedRetention,
        hub_budget: usize,
    ) -> Result<BudgetedWave, AmError> {
        // Claimed before the first read and held to the end of the wave by the
        // guard, so every wave path — both entries, either return — is counted
        // while it runs.
        let _claim = self.claim_hub_wave(bank);
        let policy = self.policy_snapshot();
        let eligible = self.eligible_hubs_under(bank, &policy)?;
        let mut report = HubDistillReport {
            eligible: eligible.clone(),
            ..HubDistillReport::default()
        };
        // PII-safe job telemetry (spirit): eligible-then-outcome counts,
        // no payloads. Errors propagate so a wave is never silently unlogged.
        self.store.record_assoc_audit(
            bank,
            "hub_distill_enqueue",
            None,
            ctx.actor.as_str(),
            &serde_json::json!({ "eligible": eligible.len() }),
        )?;
        let visited = eligible.len().min(hub_budget);
        let hubs_deferred = eligible.len() - visited;
        for hub_id in &eligible[..visited] {
            let outcome = self.distill_hub_under(bank, hub_id, ctx, retention.clone(), &policy)?;
            report.push_outcome(outcome);
        }
        // Reconcile pass: every known unit whose hub the eligible loop did not
        // visit — hubs that fell below the degree limit, stale units, and hubs
        // gone from active use. Bounded by the number of existing units, one
        // pass per wave, ordered by unit id.
        self.reconcile_units_pass(bank, ctx, retention, &policy, &eligible, &mut report)?;
        // Read after the wave's own writes have landed, so a unit this wave made
        // current is current in this count. A `source_changed` lapse this pass
        // finds is written onto its record, which is what makes it visible to
        // recall; the count below describes the answer either way, so recording
        // it cannot change this report.
        report.stale_units = self.stale_tally(bank, ctx.actor.as_str())?;
        // A wave that reconciled nothing must not overwrite the retained tally
        // with a fresh timestamp: a quiet consolidate keeps the last productive
        // wave's measurement visible instead of reading as a fresh "found
        // nothing".
        if Self::wave_reconciled_units(&report) {
            self.record_last_wave_stale(bank, report.stale_units);
        }
        self.store.record_assoc_audit(
            bank,
            "hub_distill_complete",
            None,
            ctx.actor.as_str(),
            &serde_json::json!({
                "distilled": report.distilled.len(),
                "rejected": report.rejected.len(),
                "existing": report.skipped_existing.len(),
                "revised": report.revised.len(),
                "revised_rejected": report.reconcile_rejected.len(),
                "unchanged": report.reconcile_unchanged.len(),
                "stale_kept": report.reconcile_stale_kept.len(),
                "stale": report.stale_units.total(),
                "stale_source_removed": report.stale_units.source_removed,
                "stale_source_changed": report.stale_units.source_changed,
                "stale_revision_rejected": report.stale_units.revision_rejected,
                "stale_unknown": report.stale_units.unknown,
                "hubs_deferred": hubs_deferred,
            }),
        )?;
        Ok(BudgetedWave {
            report,
            hubs_deferred,
        })
    }
}

#[cfg(test)]
#[path = "hub_distill_wave_tests.rs"]
mod hub_distill_wave_tests;
