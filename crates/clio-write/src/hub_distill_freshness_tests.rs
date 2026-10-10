// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the wave-tally retention tests: what the
//! engine records when a wave finishes, what the accessor answers before any
//! productive wave, that one bank's tally never answers for another, and that
//! the retained map stays bounded.
//!
//! ## Owns
//! - Test-only coverage of `record_last_wave_stale` and `last_wave_stale`,
//!   including the no-op-wave rule and the bounded retention behind them
//!
//! ## Does not own
//! - The stale count itself (`hub_distill_freshness`)
//! - The wave envelope and reconcile pass (`hub_distill`,
//!   `hub_distill_reconcile`)
//!
//! ## Boundary
//!
//! Changes to how a finished wave's tally is retained should normally require
//! changes here. Changes to how staleness is counted should not require changes
//! here either.
//!
//! Keep this module focused on the retained-tally seam.

use super::super::hub_distill_tests::{ctx, engine, engine_at, seed_hub, store};
use crate::hub_distill_model::StaleTally;
use clio_config::retention::ResolvedRetention;

#[test]
fn last_wave_stale_is_none_before_any_wave() {
    let store = store();
    let engine = engine(&store, 0.0, 2);
    assert_eq!(engine.last_wave_stale("bank"), None);
}

#[test]
fn a_wave_that_reconciles_nothing_keeps_the_retained_tally() {
    let store = store();
    let engine = engine_at(&store, 0.0, 2, 1_000);
    let tally = StaleTally {
        source_removed: 5,
        source_changed: 0,
        revision_rejected: 0,
        unknown: 0,
    };
    engine.record_last_wave_stale("bank", tally);
    // A quiet wave: no hubs above the degree limit and no distilled units, so
    // the wave files zero outcomes and must not overwrite the retained record.
    engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .expect("quiet wave");
    assert_eq!(
        engine.last_wave_stale("bank"),
        Some((tally, 1_000)),
        "an empty wave must not overwrite a retained record with a fresh timestamp"
    );
}

#[test]
fn record_last_wave_stale_stores_tally_and_wave_clock() {
    let store = store();
    let engine = engine_at(&store, 0.0, 2, 4_242);
    let tally = StaleTally {
        source_removed: 1,
        source_changed: 2,
        revision_rejected: 3,
        unknown: 4,
    };
    engine.record_last_wave_stale("bank", tally);
    assert_eq!(engine.last_wave_stale("bank"), Some((tally, 4_242)));
    assert_eq!(
        engine.last_wave_stale("other"),
        None,
        "one bank's tally must not answer for another"
    );
}

#[test]
fn finished_wave_records_its_stale_tally() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .expect("wave");
    assert_eq!(
        engine.last_wave_stale("bank"),
        Some((report.stale_units, 0)),
        "a finished wave records the tally it reported at its pinned clock"
    );
}

#[test]
fn the_retained_tally_map_is_bounded_and_evicts_the_oldest() {
    let store = store();
    let engine = engine_at(&store, 0.0, 2, 7);
    let tally = StaleTally::default();
    for i in 0..65 {
        engine.record_last_wave_stale(&format!("bank-{i}"), tally);
    }
    assert_eq!(
        engine.last_wave_stale("bank-0"),
        None,
        "the 65th distinct bank evicts the bank inserted first"
    );
    assert_eq!(engine.last_wave_stale("bank-1"), Some((tally, 7)));
    assert_eq!(engine.last_wave_stale("bank-64"), Some((tally, 7)));
}
