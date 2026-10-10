// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the recency factors: the exact
//! boundary constants, the invalid-input refusals, the NaN guards, the
//! monotonicity in age, and the stay within `[0, 1]` over a sweep.
//!
//! ## Owns
//! - Every assertion about `half_life_factor`, `floored_half_life_factor`,
//!   and `ramp_factor` behaviour
//!
//! ## Does not own
//! - The factor arithmetic itself (see `decay`)
//!
//! ## Boundary
//!
//! Changes to the factors' behaviour should normally require changes here
//! only as new or corrected assertions. Changes to callers should not
//! require changes here.
//!
//! Keep this module focused on proving the factors and their boundaries.

use super::{floored_half_life_factor, half_life_factor, ramp_factor};

/// The tolerance for mid-range exponential results. `f64::exp` follows IEEE
/// 754 but has unspecified precision, so a value such as the one-half-life
/// factor is asserted near `0.5` rather than bit-exact; the `0.0`, `1.0`, and
/// exact-ratio constants are asserted exactly.
const EXP_TOLERANCE: f64 = 1e-12;

/// Whether `got` is within [`EXP_TOLERANCE`] of `want`, naming both values in
/// the failure message.
fn assert_near(got: f64, want: f64, what: &str) {
    assert!(
        (got - want).abs() <= EXP_TOLERANCE,
        "{what}: got {got}, want {want}"
    );
}

/// Zero and negative age mean nothing has decayed yet, so the half-life
/// factor is exactly one.
#[test]
fn half_life_factor_returns_one_for_a_non_positive_age() {
    assert_eq!(half_life_factor(0.0, 30.0), 1.0, "zero age");
    assert_eq!(half_life_factor(-1.0, 30.0), 1.0, "one day before");
    assert_eq!(half_life_factor(-1_000.0, 30.0), 1.0, "far before");
}

/// One half-life leaves exactly half the weight, and each further half-life
/// halves it again: the definition the callers rely on.
#[test]
fn half_life_factor_halves_once_per_half_life() {
    assert_near(half_life_factor(30.0, 30.0), 0.5, "one half-life");
    assert_near(half_life_factor(60.0, 30.0), 0.25, "two half-lives");
    assert_near(half_life_factor(90.0, 30.0), 0.125, "three half-lives");
}

/// A half-life that is zero or negative has no decay meaning, and `0.0` is
/// the fail-soft answer. The span check comes first, so it wins over the
/// non-positive-age rule: `half_life_factor(0.0, 0.0)` is `0.0`, not `1.0`.
#[test]
fn half_life_factor_returns_zero_for_a_non_positive_half_life() {
    assert_eq!(half_life_factor(90.0, 0.0), 0.0, "zero half-life");
    assert_eq!(half_life_factor(90.0, -1.0), 0.0, "negative half-life");
    assert_eq!(half_life_factor(0.0, 0.0), 0.0, "invalid half-life wins");
    assert_eq!(half_life_factor(0.0, -1.0), 0.0, "invalid half-life wins");
}

/// A non-finite half-life is as unusable as a non-positive one, so all three
/// yield the same `0.0`.
#[test]
fn half_life_factor_returns_zero_for_a_non_finite_half_life() {
    assert_eq!(half_life_factor(90.0, f64::NAN), 0.0, "NaN half-life");
    assert_eq!(half_life_factor(90.0, f64::INFINITY), 0.0, "infinite");
    assert_eq!(
        half_life_factor(90.0, f64::NEG_INFINITY),
        0.0,
        "negative inf"
    );
}

/// A NaN age is not a real age, and `f64::clamp` would return NaN unchanged,
/// so the guard answers `0.0` to keep the result inside `[0, 1]`.
#[test]
fn half_life_factor_returns_zero_for_a_nan_age() {
    assert_eq!(half_life_factor(f64::NAN, 30.0), 0.0);
    assert_eq!(half_life_factor(f64::NAN, -1.0), 0.0, "span check first");
}

/// An infinite age, or one so large the exponential underflows, decays all
/// the way to zero: the clamp cannot hold the factor above it.
#[test]
fn half_life_factor_returns_zero_for_an_infinite_age() {
    assert_eq!(half_life_factor(f64::INFINITY, 30.0), 0.0, "infinite age");
    assert_eq!(half_life_factor(1.0e9, 30.0), 0.0, "underflow to zero");
}

/// The factor only ever decreases as the age grows: an implementation that
/// rose, plateaued early, or jumped would break the ordering the scoring
/// relies on.
#[test]
fn half_life_factor_decreases_monotonically_with_age() {
    let ages = [0.0, 1.0, 7.0, 30.0, 60.0, 90.0, 365.0, 1.0e6];
    for window in ages.windows(2) {
        let earlier = half_life_factor(window[0], 30.0);
        let later = half_life_factor(window[1], 30.0);
        assert!(
            earlier >= later - EXP_TOLERANCE,
            "age {} gives {earlier}, age {} gives {later}",
            window[0],
            window[1]
        );
    }
}

/// The floored factor only ever decreases as the age grows too: the floor
/// shifts the curve but cannot break the ordering the scoring relies on.
/// The grid spans the floors and the boundary ages the other floored tests
/// pin, including the underflow point where only the floor survives.
#[test]
fn floored_half_life_factor_decreases_monotonically_with_age() {
    let ages = [0.0, 1.0, 7.0, 30.0, 60.0, 90.0, 365.0, 1.0e9];
    for floor in [0.25, 0.75] {
        for window in ages.windows(2) {
            let earlier = floored_half_life_factor(window[0], 30.0, floor);
            let later = floored_half_life_factor(window[1], 30.0, floor);
            assert!(
                earlier >= later - EXP_TOLERANCE,
                "floor {floor}: age {} gives {earlier}, age {} gives {later}",
                window[0],
                window[1]
            );
        }
    }
}

/// Every factor stays inside `[0, 1]` across a sweep of ages and half-lives,
/// including the invalid ones: the contract the callers multiply with.
#[test]
fn half_life_factor_stays_within_the_unit_range() {
    let half_lives = [-1.0, 0.0, 1.0, 7.0, 30.0, 1.0e6, f64::NAN];
    let ages = [
        -100.0,
        -1.0,
        0.0,
        0.5,
        1.0,
        30.0,
        365.0,
        1.0e9,
        f64::INFINITY,
    ];
    for half_life in half_lives {
        for age in ages {
            let factor = half_life_factor(age, half_life);
            assert!(
                (0.0..=1.0).contains(&factor),
                "half_life_factor({age}, {half_life}) left the range: {factor}"
            );
        }
    }
}

/// The floored factor still starts at one: with any valid floor, age zero has
/// decayed nothing.
#[test]
fn floored_half_life_factor_returns_one_for_a_non_positive_age() {
    assert_eq!(floored_half_life_factor(0.0, 30.0, 0.25), 1.0, "zero age");
    assert_eq!(
        floored_half_life_factor(-1.0, 30.0, 0.25),
        1.0,
        "one day before"
    );
}

/// Each half-life halves only the decaying share, not the whole factor: with
/// a floor of `0.25`, one half-life leaves `0.25 + 0.75 / 2` and three leave
/// `0.25 + 0.75 / 8`. Both are binary-exact values.
#[test]
fn floored_half_life_factor_halves_the_decaying_share_per_half_life() {
    assert_eq!(
        floored_half_life_factor(30.0, 30.0, 0.25),
        0.625,
        "one half-life"
    );
    assert_eq!(
        floored_half_life_factor(90.0, 30.0, 0.25),
        0.34375,
        "three half-lives"
    );
}

/// A very large or infinite age underflows the exponential to exactly zero
/// under IEEE 754 binary64, so the result is exactly the floor. This mirrors
/// the existing factor test that pins the same underflow at `1.0e9`.
#[test]
fn floored_half_life_factor_reaches_the_floor_for_a_very_large_age() {
    assert_eq!(
        floored_half_life_factor(1.0e9, 30.0, 0.25),
        0.25,
        "underflow"
    );
    assert_eq!(
        floored_half_life_factor(f64::INFINITY, 30.0, 0.25),
        0.25,
        "infinite age"
    );
}

/// A floor that is NaN or infinite has no meaning, so the fail-soft answer is
/// `0.0`, the same value the plain factor gives an invalid half-life.
#[test]
fn floored_half_life_factor_returns_zero_for_a_non_finite_floor() {
    assert_eq!(
        floored_half_life_factor(30.0, 30.0, f64::NAN),
        0.0,
        "NaN floor"
    );
    assert_eq!(
        floored_half_life_factor(30.0, 30.0, f64::INFINITY),
        0.0,
        "infinite floor"
    );
    assert_eq!(
        floored_half_life_factor(30.0, 30.0, f64::NEG_INFINITY),
        0.0,
        "negative inf floor"
    );
}

/// A floor outside `[0, 1]` clamps to the nearest end and never panics: floor
/// zero makes the function exactly the plain factor, and floor one or above
/// makes it the constant `1.0`.
#[test]
fn floored_half_life_factor_clamps_the_floor_to_the_unit_range() {
    let plain = half_life_factor(90.0, 30.0);
    assert_eq!(
        floored_half_life_factor(90.0, 30.0, 0.0),
        plain,
        "floor zero"
    );
    assert_eq!(
        floored_half_life_factor(90.0, 30.0, -0.1),
        plain,
        "floor below zero"
    );
    assert_eq!(floored_half_life_factor(90.0, 30.0, 1.0), 1.0, "floor one");
    assert_eq!(
        floored_half_life_factor(90.0, 30.0, 2.0),
        1.0,
        "floor above one"
    );
}

/// An invalid half-life or a NaN age leaves exactly the floor: the plain
/// factor answers `0.0`, so only the floor survives.
#[test]
fn floored_half_life_factor_yields_the_floor_for_an_invalid_decay_input() {
    for half_life in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            floored_half_life_factor(90.0, half_life, 0.25),
            0.25,
            "half-life {half_life}"
        );
    }
    assert_eq!(
        floored_half_life_factor(f64::NAN, 30.0, 0.25),
        0.25,
        "NaN age"
    );
}

/// Every floored factor stays inside `[clamped floor, 1]` across a sweep of
/// floors and ages, including the invalid inputs: the contract callers
/// multiply with.
#[test]
fn floored_half_life_factor_stays_between_the_floor_and_one() {
    let floors: [f64; 6] = [-1.0, 0.0, 0.25, 0.75, 1.0, 2.0];
    let ages = [
        -100.0,
        -1.0,
        0.0,
        0.5,
        1.0,
        30.0,
        90.0,
        365.0,
        1.0e9,
        f64::INFINITY,
    ];
    for floor in floors {
        let lower = floor.clamp(0.0, 1.0);
        for age in ages {
            let factor = floored_half_life_factor(age, 30.0, floor);
            assert!(
                (lower..=1.0).contains(&factor),
                "floored_half_life_factor({age}, 30.0, {floor}) left the range: {factor}"
            );
        }
    }
}

/// Half the span is exactly half: the ratio `45 / 90` is representable
/// exactly, so this boundary constant is asserted bit-exact.
#[test]
fn ramp_factor_at_half_the_span_is_one_half() {
    assert_eq!(ramp_factor(45.0, 90.0), 0.5, "half the span");
    assert_eq!(ramp_factor(90.0, 90.0), 1.0, "exactly the span");
}

/// A span that is zero or negative has no ramp meaning, so the answer is
/// `0.0` whatever the age.
#[test]
fn ramp_factor_returns_zero_for_a_non_positive_span() {
    assert_eq!(ramp_factor(45.0, 0.0), 0.0, "zero span");
    assert_eq!(ramp_factor(45.0, -1.0), 0.0, "negative span");
    assert_eq!(ramp_factor(0.0, 0.0), 0.0, "invalid span wins");
}

/// A non-finite span is as unusable as a non-positive one, so all three
/// yield the same `0.0`.
#[test]
fn ramp_factor_returns_zero_for_a_non_finite_span() {
    assert_eq!(ramp_factor(45.0, f64::NAN), 0.0, "NaN span");
    assert_eq!(ramp_factor(45.0, f64::INFINITY), 0.0, "infinite span");
    assert_eq!(ramp_factor(45.0, f64::NEG_INFINITY), 0.0, "negative inf");
}

/// A NaN age is not a real age, and `f64::clamp` would return NaN unchanged,
/// so the guard answers `0.0` to keep the result inside `[0, 1]`.
#[test]
fn ramp_factor_returns_zero_for_a_nan_age() {
    assert_eq!(ramp_factor(f64::NAN, 90.0), 0.0);
    assert_eq!(ramp_factor(f64::NAN, -1.0), 0.0, "span check first");
}

/// A negative age clamps to the bottom of the range, and any age at or past
/// the span clamps to the top: the ramp is flat outside its window.
#[test]
fn ramp_factor_clamps_outside_the_window() {
    assert_eq!(ramp_factor(-5.0, 90.0), 0.0, "negative age");
    assert_eq!(ramp_factor(200.0, 90.0), 1.0, "past the span");
    assert_eq!(ramp_factor(f64::INFINITY, 90.0), 1.0, "infinite age");
    assert_eq!(ramp_factor(1.0e9, 90.0), 1.0, "far past the span");
}

/// The factor only ever grows as the age grows, then stays flat: an
/// implementation that dipped or wrapped would break the ordering the
/// scoring relies on.
#[test]
fn ramp_factor_grows_monotonically_with_age() {
    let ages = [-10.0, 0.0, 1.0, 45.0, 89.0, 90.0, 91.0, 200.0];
    for window in ages.windows(2) {
        let earlier = ramp_factor(window[0], 90.0);
        let later = ramp_factor(window[1], 90.0);
        assert!(
            earlier <= later + EXP_TOLERANCE,
            "age {} gives {earlier}, age {} gives {later}",
            window[0],
            window[1]
        );
    }
}

/// Every factor stays inside `[0, 1]` across a sweep of ages and spans,
/// including the invalid ones: the contract the callers multiply with.
#[test]
fn ramp_factor_stays_within_the_unit_range() {
    let spans = [-1.0, 0.0, 1.0, 30.0, 90.0, 1.0e6, f64::NAN];
    let ages = [
        -100.0,
        -1.0,
        0.0,
        0.5,
        45.0,
        90.0,
        200.0,
        1.0e9,
        f64::INFINITY,
    ];
    for span in spans {
        for age in ages {
            let factor = ramp_factor(age, span);
            assert!(
                (0.0..=1.0).contains(&factor),
                "ramp_factor({age}, {span}) left the range: {factor}"
            );
        }
    }
}
