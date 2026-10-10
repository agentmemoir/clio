// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the end-to-end evidence that `consolidate
//! --force` performs the full forced wave: the dirty-path refresh plus hub
//! reconciliation, so one call leaves a lapsed consolidated unit current and
//! reports it.
//!
//! ## Owns
//! - The forced-wave and no-unit scenarios over the real CLI
//!
//! ## Does not own
//! - The fixture and the readers (`consolidate_wave_support`)
//! - The bank-selection surface (`consolidate_bank_harness`)
//!
//! ## Boundary
//!
//! Changes to what a forced wave reconciles should normally require changes
//! here. Changes to the hub algorithm itself should not require changes here.
//!
//! Keep this module focused on the forced wave's effect.

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{
    Fixture, TUNABLE_ITEMS, claim_set, current_list, distilled_bank, entry_ids, summary_count,
};
use serde_json::Value;

/// Withdraw one claimed source of `unit` and return it: the cascade marks every
/// unit that claimed it stale, which is the lapse a wave has to rebuild.
fn withdraw_a_claimed_source(f: &Fixture, unit: &str) -> String {
    let (_list, _ids, neighbour) = claim_set(f, unit);
    f.assert_ok(&[
        "discard",
        &neighbour,
        "--reason",
        "operator withdrew it",
        "--confirm",
        "--actor",
        "harness",
        "--output",
        "json",
    ]);
    assert_eq!(
        f.stale_reason_of(unit),
        "source_removed",
        "the discard cascade marks the claiming unit stale"
    );
    neighbour
}

#[test]
fn force_reconciles_a_lapsed_unit_and_the_report_says_so() {
    let (f, unit) = distilled_bank();
    let neighbour = withdraw_a_claimed_source(&f, &unit);
    let revisions_before = f.evidence_trail(&unit).len();

    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert!(
        summary_count(summary, "revised").is_some_and(|n| n >= 1),
        "the rebuild must be reported: {summary}"
    );
    assert!(
        summary_count(summary, "stale_kept").is_some_and(|n| n >= 1),
        "the withdrawn hub's own unit is the documented stale exception: {summary}"
    );
    assert_eq!(
        f.stale_reason_of(&unit),
        "",
        "the forced wave leaves the unit current: {summary}"
    );
    assert_eq!(
        f.evidence_trail(&unit).len(),
        revisions_before + 1,
        "one revision records the re-derivation"
    );
    let current = current_list(&f.evidence_trail(&unit));
    assert!(
        !entry_ids(&current).contains(&neighbour),
        "the withdrawn source is gone from the claim set"
    );
}

#[test]
fn consolidate_without_force_keeps_the_normal_wave_reconcile_rule() {
    let (f, unit) = distilled_bank();
    withdraw_a_claimed_source(&f, &unit);

    let payload = f.json(&["consolidate", "--output", "json"]);
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    // The wave's normal reconcile rule reaches a settled unit on the unforced
    // wave as well; `force` is the guarantee that the wave also performs the
    // dirty-path refresh, not the only way a unit is rebuilt.
    assert_eq!(
        f.stale_reason_of(&unit),
        "",
        "the normal wave rule applies without force"
    );
}

#[test]
fn force_on_a_bank_without_consolidated_units_is_an_honest_no_op() {
    let f = Fixture::new();
    // Degree 5 is below the default limit of 16, so the bank has items and
    // edges but no consolidated unit at all.
    f.seed_coactivated_bank(TUNABLE_ITEMS);

    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert_eq!(summary_count(summary, "distilled"), Some(0), "{summary}");
    assert_eq!(summary_count(summary, "stale"), Some(0), "{summary}");
    assert!(
        f.distilled_ids().is_empty(),
        "no unit may be written: {summary}"
    );
}
