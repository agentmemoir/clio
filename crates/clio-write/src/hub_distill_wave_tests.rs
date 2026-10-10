// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-wave hub budget tests: a wave's hub
//! loop stops at the budget the caller supplies, a wave that stops records the
//! eligible hubs it left behind, and every hub it defers is still eligible for
//! the next wave.
//!
//! ## Owns
//! - Test-only coverage of `consolidate_hubs_budgeted` and its
//!   `BudgetedWave` result
//! - The three-hub fixture these budget tests share
//!
//! ## Does not own
//! - What a finished wave records and how a running wave is counted
//!   (`hub_distill_wave_tests_more_tests`)
//! - The wave envelope itself (`hub_distill_wave`)
//! - Hub seeding and engine fixtures (`hub_distill_tests`)
//!
//! ## Boundary
//!
//! Changes to how many hubs a wave may distill, or to what a wave leaves behind
//! for the next one, should normally require changes here. Changes to how a
//! single hub is distilled should not require changes here.
//!
//! Keep this module focused on the hub budget seam.

use super::super::hub_distill_tests::{ctx, engine, seed_hub, store};
use clio_config::retention::ResolvedRetention;
use clio_store::{SqliteStore, Store};
use std::sync::Arc;

/// Seed three disjoint over-degree hub clusters, each eligible on its own, so
/// a budget that binds can only be the budget and not a missing hub.
fn three_hubs() -> Arc<SqliteStore> {
    let store = store();
    for (hub, prefix) in [("hub1", "n1"), ("hub2", "n2"), ("hub3", "n3")] {
        let neighbors = [
            format!("{prefix}a"),
            format!("{prefix}b"),
            format!("{prefix}c"),
        ];
        let refs: Vec<&str> = neighbors.iter().map(String::as_str).collect();
        seed_hub(&store, "bank", hub, &refs, 0.15);
    }
    store
}

#[test]
fn wave_stops_at_the_hub_budget_and_records_what_it_left_behind() {
    let store = three_hubs();
    let engine = engine(&store, 0.0, 2);
    let wave = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 1)
        .unwrap();
    assert_eq!(
        wave.report.eligible,
        vec!["hub1".to_owned(), "hub2".to_owned(), "hub3".to_owned()]
    );
    assert_eq!(wave.report.distilled.len(), 1, "{wave:?}");
    assert_eq!(wave.hubs_deferred, 2, "{wave:?}");
}

#[test]
fn an_unbounded_wave_defers_no_eligible_hub() {
    let store = three_hubs();
    let engine = engine(&store, 0.0, 2);
    let wave = engine
        .consolidate_hubs_budgeted(
            "bank",
            &ctx("bank"),
            &ResolvedRetention::default(),
            usize::MAX,
        )
        .unwrap();
    assert_eq!(wave.hubs_deferred, 0, "{wave:?}");
    assert_eq!(wave.report.distilled.len(), 3, "{wave:?}");
}

#[test]
fn a_zero_budget_defers_every_eligible_hub() {
    let store = three_hubs();
    let engine = engine(&store, 0.0, 2);
    let wave = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &ResolvedRetention::default(), 0)
        .unwrap();
    assert_eq!(wave.hubs_deferred, 3, "{wave:?}");
    assert!(wave.report.distilled.is_empty(), "{wave:?}");
    assert_eq!(wave.report.eligible.len(), 3, "{wave:?}");
}

#[test]
fn the_hubs_a_wave_left_behind_are_distilled_by_the_next_wave() {
    let store = three_hubs();
    let engine = engine(&store, 0.0, 2);
    let retention = ResolvedRetention::default();
    let first = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, 1)
        .unwrap();
    assert_eq!(
        first.report.distilled,
        vec!["hub-distill:bank:hub1"],
        "{first:?}"
    );
    assert_eq!(first.hubs_deferred, 2, "{first:?}");

    let second = engine
        .consolidate_hubs_budgeted("bank", &ctx("bank"), &retention, 3)
        .unwrap();
    assert_eq!(second.hubs_deferred, 0, "{second:?}");
    for hub in ["hub2", "hub3"] {
        let item_id = format!("hub-distill:bank:{hub}");
        assert!(
            second.report.distilled.contains(&item_id),
            "{hub} deferred by the first wave was not distilled: {second:?}"
        );
        assert!(
            store.get_memory_item("bank", &item_id).unwrap().is_some(),
            "{hub} has no distilled item in the store"
        );
    }
}

#[cfg(test)]
#[path = "hub_distill_wave_tests_more_tests.rs"]
mod hub_distill_wave_tests_more_tests;
