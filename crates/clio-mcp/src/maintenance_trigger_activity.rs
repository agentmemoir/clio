// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance trigger's active-bank
//! registry: recording the banks the runtime resolved, and selecting the banks
//! one sweeper tick evaluates.
//!
//! ## Owns
//! - `MaintenanceTriggerState::note_bank`: record a resolved bank
//! - `MaintenanceTriggerState::active_banks`: the recorded-bank snapshot
//! - `MaintenanceTriggerState::claim_tick_banks`: the per-tick bank selection
//! - `MaintenanceTriggerState::retain_pressured`: keep only banks still needing
//!   maintenance
//!
//! ## Does not own
//! - The per-bank decision and record (`maintenance_trigger`)
//! - The sweeper loop and its cadence (`runtime_index`)
//!
//! ## Boundary
//!
//! Changes to which banks a tick evaluates should normally require changes
//! here. Changes to the per-bank decision should not.
//!
//! Keep this module focused on the active-bank registry and tick selection.

use super::MaintenanceTriggerState;
use crate::runtime::SHARED_BANK;
use std::collections::HashSet;

impl MaintenanceTriggerState {
    /// Note that the runtime resolved this bank for a caller, so the next tick
    /// evaluates its pressure. Called from the one bank-resolution choke point.
    pub fn note_bank(&self, bank: &str) {
        if bank.is_empty() {
            return;
        }
        self.active
            .lock()
            .expect("trigger active mutex poisoned")
            .insert(bank.to_owned());
    }

    /// The active-bank snapshot, sorted so a reader sees one stable order.
    #[must_use]
    pub fn active_banks(&self) -> Vec<String> {
        let mut banks: Vec<String> = self
            .active
            .lock()
            .expect("trigger active mutex poisoned")
            .iter()
            .cloned()
            .collect();
        banks.sort_unstable();
        banks
    }

    /// Claim the banks one tick evaluates: every active bank plus the default
    /// bank, and the shared bank when the deployment enabled it. Never the
    /// whole store, so an idle tick issues a bounded number of bank reads.
    ///
    /// Taking the active set empties the registry before evaluation, so a bank
    /// resolved while the tick runs is recorded into the now-empty registry and
    /// survives the [`Self::retain_pressured`] merge that ends the tick. The
    /// caller hands back the banks that still showed pressure.
    pub(super) fn claim_tick_banks(&self, default_bank: &str, shared_enabled: bool) -> Vec<String> {
        let mut banks: HashSet<String> =
            std::mem::take(&mut *self.active.lock().expect("trigger active mutex poisoned"));
        banks.insert(default_bank.to_owned());
        if shared_enabled {
            banks.insert(SHARED_BANK.to_owned());
        }
        let mut banks: Vec<String> = banks.into_iter().collect();
        banks.sort_unstable();
        banks
    }

    /// Put the banks that still showed pressure back into the registry, so the
    /// next tick keeps evaluating them until they are quiet. A bank resolved
    /// during the tick is already in the registry and is left alone.
    pub(super) fn retain_pressured(&self, banks: impl IntoIterator<Item = String>) {
        let mut active = self.active.lock().expect("trigger active mutex poisoned");
        for bank in banks {
            active.insert(bank);
        }
    }
}
