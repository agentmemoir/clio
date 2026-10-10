// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving, through the crate's public API, that
//! the admission recency value moved exactly as documented: the old plain
//! exponential's values at 7, 30, 90, and 365 days against the half-life
//! curve's values at the same ages.
//!
//! ## Owns
//! - The documented before/after recency movement over
//!   `clio_admission::factors::recency_decay`
//!
//! ## Does not own
//! - The curve arithmetic (see `clio_types::decay::half_life_factor`)
//! - The in-crate factor assertions (see `src/factors_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to the documented recency movement should normally require changes
//! here. Changes to the factor signals' unit-level expectations should not
//! require changes here.
//!
//! Keep this module focused on the documented recency movement contract.

use clio_admission::factors::recency_decay;

/// The published admission row at `tau = 30` days: age in days and the
/// half-life curve value the documentation lists for it.
const DOCUMENTED_ROW: [(f64, f64); 4] = [
    (7.0, 0.850_667),
    (30.0, 0.500_000),
    (90.0, 0.125_000),
    (365.0, 0.000_218),
];

#[test]
fn a_missing_prior_keeps_the_full_recency_value() {
    assert_eq!(recency_decay(None, 30.0), 1.0);
}

#[test]
fn one_half_life_moved_from_the_old_formula_to_one_half() {
    // The documented movement at 30 days: the old `exp(-age / 30)` curve scored
    // 0.367879, and the half-life curve now scores 0.5.
    let old_formula = (-30.0_f64 / 30.0).exp();
    assert!(
        (old_formula - 0.367_879).abs() < 1e-6,
        "old formula at 30 days: expected 0.367879, got {old_formula}"
    );
    let now = recency_decay(Some(30.0), 30.0);
    assert!(
        (now - 0.5).abs() < 1e-12,
        "moved curve at 30 days: expected 0.5, got {now}"
    );
}

#[test]
fn the_admission_curve_matches_the_documented_row() {
    for (age, documented) in DOCUMENTED_ROW {
        let got = recency_decay(Some(age), 30.0);
        assert!(
            (got - documented).abs() < 1e-6,
            "moved curve at {age} days: expected {documented}, got {got}"
        );
    }
}
