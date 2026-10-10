// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the wave's freshness pass: how many of the
//! bank's units are not current right now, by reason, and the one lapse the pass
//! records on the record it is already re-reading.
//!
//! ## Owns
//! - `HubDistillEngine::stale_tally`, the by-reason count and the
//!   `source_changed` write
//! - The retained wave tally: `record_last_wave_stale` and `last_wave_stale`,
//!   the per-bank bounded map that keeps the last productive wave's
//!   measurement, and the rule that a wave reconciling nothing never records
//!
//! ## Does not own
//! - The wave envelope and the reconcile pass (`hub_distill`,
//!   `hub_distill_reconcile`)
//! - The freshness decision itself and the record's stale state
//!   (`clio-store::distilled_freshness`, `clio-store::distilled_unit`)
//! - The report shape (`hub_distill_model`)
//!
//! ## Boundary
//!
//! Changes to what a wave reports about its bank's currency, to which lapse it
//! records, or to how the finished tally is retained between waves should
//! normally require changes here. Changes to how staleness is
//! decided should not require changes here; changes to what reconciliation does
//! with a recorded reason should not require changes here either.
//!
//! Keep this module focused on counting and recording freshness at wave time.

use super::HubDistillEngine;
use clio_store::{STALE_SOURCE_CHANGED, unit_freshness};
use clio_types::AmError;
use std::collections::{HashMap, VecDeque};

use crate::hub_distill_model::{HubDistillReport, StaleTally};

/// The retained per-bank wave tally, bounded at 64 banks.
///
/// The only writer is a finished wave, so growth is one permanent entry per
/// distinct bank string ever waved, for the process lifetime. The cap bounds
/// that: an insert past 64 entries evicts the bank inserted first. Insertion
/// order is kept in a queue alongside the map, and re-recording a bank keeps
/// its original slot, so the eviction order is the first-waved order, not the
/// last-waved order.
#[derive(Default)]
pub(super) struct RetainedWaveTallies {
    map: HashMap<String, (StaleTally, i64)>,
    order: VecDeque<String>,
}

impl RetainedWaveTallies {
    /// How many banks the map retains before the oldest-inserted one is
    /// evicted.
    const CAP: usize = 64;

    fn insert(&mut self, bank: &str, value: (StaleTally, i64)) {
        if !self.map.contains_key(bank) {
            self.order.push_back(bank.to_owned());
            while self.map.len() >= Self::CAP {
                match self.order.pop_front() {
                    Some(oldest) => {
                        self.map.remove(&oldest);
                    }
                    None => break,
                }
            }
        }
        self.map.insert(bank.to_owned(), value);
    }

    fn get(&self, bank: &str) -> Option<(StaleTally, i64)> {
        self.map.get(bank).copied()
    }
}

impl HubDistillEngine {
    /// How many of the bank's units are not current right now, by reason.
    ///
    /// One `unit_freshness` call per unit, so the store's single freshness rule
    /// decides every count and a unit whose sources changed in a way the wave's
    /// own text comparison did not notice is still reported. A unit the store
    /// cannot judge counts as unknown: it is never reported as current.
    ///
    /// A `source_changed` lapse is also **written** here, onto the record this
    /// pass already re-reads, and the next successful revision clears it. The
    /// reason is otherwise decided on read and never stored, and a retrieve call
    /// reads only the item's snapshot — so without this write recall cannot see
    /// the lapse and keeps treating the unit as a current parent. A unit already
    /// carrying a reason is left alone: that determination outranks a moved stamp,
    /// and re-writing an unchanged row costs a dead row version per unit per wave.
    ///
    /// The write is a compare-and-set against the record this pass judged, so a
    /// discard cascade that commits while the pass is between units wins: the
    /// withdrawn source stays out of the claim set and the unit keeps
    /// `source_removed` instead of being handed back a weaker reason and a source
    /// that left active use. A refused write changes nothing else: the tally
    /// still counts what this pass measured, and the next wave sees the unit.
    ///
    /// # Errors
    /// Storage failures, including on the write this makes.
    pub(super) fn stale_tally(&self, bank: &str, actor: &str) -> Result<StaleTally, AmError> {
        let mut tally = StaleTally::default();
        for record in self.store.list_distilled_units(bank)? {
            let freshness = unit_freshness(self.store.as_ref(), bank, &record)?;
            tally.add(&freshness);
            if freshness.stale_reason.as_deref() != Some(STALE_SOURCE_CHANGED) {
                continue;
            }
            let mut updated = record.clone();
            if updated.mark_source_changed(&self.now_millis().to_string()) {
                self.store
                    .update_distilled_unit_if_unchanged(&record, &updated, actor)?;
            }
        }
        Ok(tally)
    }

    /// Record the freshness tally a finished wave just published for `bank`.
    ///
    /// Called only for a wave that reconciled at least one unit: a wave that
    /// reconciled nothing never records, so a quiet consolidate cannot overwrite
    /// a retained record with a fresh timestamp. The timestamp is the same
    /// pinned wave clock the wave itself read. The guard is held only for the
    /// insert.
    pub(super) fn record_last_wave_stale(&self, bank: &str, tally: StaleTally) {
        let now = self.now_millis();
        self.last_wave_stale
            .lock()
            .expect("hub distill tally")
            .insert(bank, (tally, now));
    }

    /// Whether the finished wave reconciled at least one unit: it filed at
    /// least one outcome, so it visited a hub or an existing distilled unit,
    /// even if every visit wrote nothing. Only a wave with an empty eligible
    /// list and no existing distilled units reconciles nothing.
    pub(super) fn wave_reconciled_units(report: &HubDistillReport) -> bool {
        let visited = report.distilled.len()
            + report.rejected.len()
            + report.skipped_existing.len()
            + report.revised.len()
            + report.reconcile_rejected.len()
            + report.reconcile_unchanged.len()
            + report.reconcile_stale_kept.len();
        visited > 0
    }

    /// The last productive wave's tally for `bank`, with the epoch milliseconds
    /// that wave ran at, or `None` when no wave that reconciled at least one
    /// unit has finished for the bank yet.
    ///
    /// Process-local: a restart forgets every recorded tally, and a bank whose
    /// waves all reconciled nothing answers `None` rather than a fabricated
    /// current count. A no-op wave never clears or rewrites a retained record,
    /// so the value is always a measurement a real wave took. It is a wave-time
    /// snapshot; it is not recomputed on read, and reading it performs no store
    /// work.
    #[must_use]
    pub fn last_wave_stale(&self, bank: &str) -> Option<(StaleTally, i64)> {
        self.last_wave_stale
            .lock()
            .expect("hub distill tally")
            .get(bank)
    }
}

#[cfg(test)]
#[path = "hub_distill_freshness_tests.rs"]
mod hub_distill_freshness_tests;
