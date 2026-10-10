// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure recency factors shared by
//! scoring callers: exponential half-life decay, the same decay held above a
//! floor, and the linear ramp.
//!
//! ## Owns
//! - `half_life_factor`: `exp(-ln2 * age / half_life)`, the share of a
//!   memory's weight that survives its age, halving once per half-life
//! - `floored_half_life_factor`: the same factor lifted by a floor, so the
//!   surviving share never falls below the floor
//! - `ramp_factor`: the clamped `age / span` ratio, the linear share that
//!   grows from zero at age zero to one at the span
//!
//! ## Does not own
//! - The age itself: every caller supplies it, so nothing here reads a clock
//! - The weights these factors multiply, or any storage, logging, or I/O
//!
//! ## Boundary
//!
//! Changes to the factor arithmetic or their boundary behaviour should
//! normally require changes here. Changes to how a caller weights or stores
//! a returned factor should not require changes here.
//!
//! Keep this module focused on the three pure factor functions and nothing
//! else.

use std::f64::consts::LN_2;

/// The share of a memory's weight that survives `age_days`, under exponential
/// decay with the given `half_life_days`.
///
/// The result is `exp(-ln2 * age / half_life)` clamped to `[0, 1]`. The
/// half-life is the age at which half the weight is left: the factor is `0.5`
/// when the age equals the half-life, `0.25` at two half-lives, and so on. At
/// large ages the exponential underflows to exactly `0.0`, so callers that
/// need a non-zero floor apply their own.
///
/// Boundary behaviour, checked in this order:
///
/// - A half-life that is non-finite or not positive yields `0.0`. This is
///   checked first, so an invalid half-life wins over any age: the call
///   `half_life_factor(0.0, -1.0)` is `0.0`, not `1.0`. A non-positive
///   half-life has no decay meaning, and `0.0` matches the fail-soft value
///   retrieval uses for an invalid decay constant.
/// - A NaN age yields `0.0`. `f64::clamp` returns NaN unchanged, so without
///   this guard the result would not stay in `[0, 1]`.
/// - An age that is not positive yields `1.0`: nothing has decayed yet.
/// - An infinite age underflows the exponential to `0.0`.
///
/// Pure: a function of its two arguments only. It reads no clock,
/// configuration, or other ambient state, and does no I/O.
#[must_use]
pub fn half_life_factor(age_days: f64, half_life_days: f64) -> f64 {
    if !half_life_days.is_finite() || half_life_days <= 0.0 {
        return 0.0;
    }
    if age_days.is_nan() {
        return 0.0;
    }
    if age_days <= 0.0 {
        return 1.0;
    }
    (-age_days * LN_2 / half_life_days).exp().clamp(0.0, 1.0)
}

/// The share of a memory's weight that survives `age_days`, under exponential
/// decay with the given `half_life_days`, never falling below `floor`.
///
/// The result is `floor + (1 - floor) * half_life_factor(age, half_life)`:
/// the decaying part of the weight shrinks by half each half-life while the
/// floor holds a fixed share that never decays. At age zero the result is
/// `1.0`; with a floor of `0.25` one half-life leaves `0.625` and three
/// half-lives leave `0.34375`.
///
/// Boundary behaviour, checked in this order:
///
/// - A floor that is NaN or not finite yields `0.0`. This is checked first,
///   so an invalid floor wins over every other input, and `0.0` matches the
///   fail-soft value this module uses for an invalid constant.
/// - A floor below `0.0` clamps to `0.0`, and a floor above `1.0` clamps to
///   `1.0`, so the result stays inside `[0, 1]`.
/// - An invalid half-life yields exactly the floor: `half_life_factor`
///   answers `0.0`, and `floor + (1 - floor) * 0.0` is `floor`.
/// - A NaN age yields exactly the floor, for the same reason.
/// - An age that is not positive yields exactly `1.0`: nothing has decayed.
/// - A very large age underflows the exponential to exactly `0.0` under IEEE
///   754 binary64, so the result is exactly the floor. The factor tests in
///   this module pin that underflow at an age of `1.0e9`.
///
/// Pure: a function of its three arguments only. It reads no clock,
/// configuration, or other ambient state, and does no I/O.
#[must_use]
pub fn floored_half_life_factor(age_days: f64, half_life_days: f64, floor: f64) -> f64 {
    if !floor.is_finite() {
        return 0.0;
    }
    let floor = floor.clamp(0.0, 1.0);
    floor + (1.0 - floor) * half_life_factor(age_days, half_life_days)
}

/// The linear share of `span_days` a memory has aged through, from
/// `ramp_factor(0.0, span)` of `0.0` up to `1.0` at the span.
///
/// The result is the ratio `age / span` clamped to `[0, 1]`: it grows
/// linearly with age up to the span and stays flat at `1.0` beyond it. A
/// caller warming a score up over a fixed window multiplies by this.
///
/// Boundary behaviour:
///
/// - A span that is non-finite or not positive yields `0.0`. Such a span has
///   no ramp meaning, and `0.0` matches the fail-soft value callers use for
///   an invalid window.
/// - A NaN age yields `0.0`. `f64::clamp` returns NaN unchanged, so without
///   this guard the result would not stay in `[0, 1]`.
/// - A negative age clamps to `0.0`, and a ratio above one clamps to `1.0`.
///
/// Pure: a function of its two arguments only. It reads no clock,
/// configuration, or other ambient state, and does no I/O.
#[must_use]
pub fn ramp_factor(age_days: f64, span_days: f64) -> f64 {
    if !span_days.is_finite() || span_days <= 0.0 {
        return 0.0;
    }
    if age_days.is_nan() {
        return 0.0;
    }
    (age_days / span_days).clamp(0.0, 1.0)
}

#[cfg(test)]
#[path = "decay_tests.rs"]
mod decay_tests;
