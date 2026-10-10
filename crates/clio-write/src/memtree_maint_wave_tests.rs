// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-flight wave gauge accessor tests: the
//! accessor counts the slots `begin_wave` claims and `end_wave` releases,
//! including a wave that is scheduled but not yet running.
//!
//! ## Owns
//! - Test-only coverage of `MemTreeMaint::waves_in_flight`
//!
//! ## Does not own
//! - The gauge itself and its claim/release protocol (`memtree_maint`)
//! - Quiescence waiting and the wave body (`memtree_maint`)
//!
//! ## Boundary
//!
//! Changes to what the accessor answers should normally require changes here.
//! Changes to how a wave is run should not require changes here.
//!
//! Keep this module focused on the wave gauge accessor.

use crate::maintenance::LeafIndex;
use crate::memtree::MemForest;
use crate::memtree_maint::MemTreeMaint;
use std::sync::{Arc, Mutex};

fn maint() -> Arc<MemTreeMaint> {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    MemTreeMaint::new(forest, leaves)
}

#[test]
fn waves_in_flight_counts_claimed_and_released_wave_slots() {
    let maint = maint();
    assert_eq!(maint.waves_in_flight(), 0, "no wave scheduled yet");
    maint.begin_wave();
    maint.begin_wave();
    assert_eq!(maint.waves_in_flight(), 2, "two claimed slots");
    maint.end_wave();
    assert_eq!(maint.waves_in_flight(), 1, "one released");
    maint.end_wave();
    assert_eq!(maint.waves_in_flight(), 0, "queue drained");
}

#[test]
fn refresh_bank_releases_the_wave_slot_it_claims() {
    let maint = maint();
    let cleared = maint.refresh_bank("wave-gauge-bank");
    assert!(cleared.is_empty(), "no dirty marks were enqueued");
    assert_eq!(maint.waves_in_flight(), 0, "wave slot released");
    assert_eq!(maint.waves_started(), 1, "the wave was counted as begun");
}
