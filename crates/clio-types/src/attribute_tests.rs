// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the continuous engine and schema types.
//!
//! ## Owns
//! - formula fixtures (appendix worked example), cold start, bounds, trend window
//! - `AttributeSchema` validation and knob-resolution cases
//!
//! ## Does not own
//! - Persistence or concurrency tests (see `clio-store` attribute suites)
//!
//! ## Boundary
//!
//! Changes to formula expectations should normally update tests here.

use crate::attribute::{
    AttributeSchema, Bounds, ContinuousEngineConfig, DEFAULT_ALPHA, DEFAULT_TREND_WINDOW,
    apply_observation, mean_last_n,
};
use crate::error::ErrorCode;
use crate::triple::UpdateRule;

const EPS: f64 = 1e-9;

fn cfg() -> ContinuousEngineConfig {
    ContinuousEngineConfig {
        alpha: 0.5,
        trend_window: 3,
        bounds: None,
        clamp: false,
    }
}

/// Appendix worked fixture (α=0.5, N=3, obs 1.0, 0.0, 1.0).
#[test]
fn t02_t03_appendix_fixture() {
    let mut window: Vec<f64> = Vec::new();
    let c = cfg();
    let s0 = apply_observation(&c, &window, 1.0, "2024-01-01").unwrap();
    assert!((s0.state - 1.0).abs() < EPS);
    assert!((s0.trend - 1.0).abs() < EPS);
    assert_eq!(s0.window, vec![1.0]);
    window = s0.window;
    let s1 = apply_observation(&c, &window, 0.0, "2024-01-02").unwrap();
    assert!((s1.state - 0.5).abs() < EPS);
    assert!((s1.trend - 0.75).abs() < EPS);
    window = s1.window;
    let s2 = apply_observation(&c, &window, 1.0, "2024-01-02").unwrap();
    assert!((s2.state - 0.75).abs() < EPS);
    assert!((s2.trend - 0.75).abs() < EPS);
    assert_eq!(s2.window, vec![1.0, 0.5, 0.75]);
    assert_eq!(s2.as_of, "2024-01-02");
}

/// cold start — state and trend equal the first observation.
#[test]
fn t04_cold_start() {
    let out = apply_observation(&cfg(), &[], 0.42, "t").unwrap();
    assert!((out.state - 0.42).abs() < EPS);
    assert!((out.trend - 0.42).abs() < EPS);
}

#[test]
fn trend_shorter_than_window_uses_all_states() {
    let c = ContinuousEngineConfig {
        alpha: 1.0,
        trend_window: 5,
        bounds: None,
        clamp: false,
    };
    let s1 = apply_observation(&c, &[1.0], 3.0, "t").unwrap();
    assert!((s1.trend - 2.0).abs() < EPS, "trend={}", s1.trend);
    // Window beyond N is truncated to the last N states.
    assert_eq!(s1.window, vec![1.0, 3.0]);
}

#[test]
fn deterministic_for_identical_inputs() {
    let c = cfg();
    let a = apply_observation(&c, &[1.0, 0.5], 1.0, "t").unwrap();
    let b = apply_observation(&c, &[1.0, 0.5], 1.0, "t").unwrap();
    assert_eq!(a, b);
}

/// Bounds: reject out-of-range by default; clamp when schema opts in.
#[test]
fn bounds_reject_then_clamp() {
    let reject = ContinuousEngineConfig {
        bounds: Some(Bounds { lo: 0.0, hi: 1.0 }),
        ..cfg()
    };
    let err = apply_observation(&reject, &[], 2.0, "t").unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    let clamp = ContinuousEngineConfig {
        clamp: true,
        ..reject
    };
    // Clamped to hi=1.0, then EMA: 0.5·1.0 + 0.5·0.5 = 0.75.
    let out = apply_observation(&clamp, &[0.5], 2.0, "t").unwrap();
    assert!((out.state - 0.75).abs() < EPS, "state={}", out.state);
}

#[test]
fn non_finite_and_bad_config_rejected() {
    let c = cfg();
    assert_eq!(
        apply_observation(&c, &[], f64::NAN, "t").unwrap_err().code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        apply_observation(&c, &[], f64::INFINITY, "t")
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    let bad_alpha = ContinuousEngineConfig {
        alpha: 0.0,
        ..cfg()
    };
    assert_eq!(
        apply_observation(&bad_alpha, &[], 1.0, "t")
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        apply_observation(
            &ContinuousEngineConfig {
                trend_window: 0,
                ..cfg()
            },
            &[],
            1.0,
            "t"
        )
        .unwrap_err()
        .code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn mean_last_n_and_defaults() {
    assert!((mean_last_n(&[1.0, 0.5, 0.75], 3) - 0.75).abs() < EPS);
    assert!((mean_last_n(&[1.0, 0.5], 3) - 0.75).abs() < EPS);
    assert!(mean_last_n(&[], 3).is_nan());
    let d = ContinuousEngineConfig::defaults();
    assert!((d.alpha - DEFAULT_ALPHA).abs() < EPS);
    assert_eq!(d.trend_window, DEFAULT_TREND_WINDOW);
    assert_eq!(d.trend_window, 10);
    assert!((d.alpha - 0.3).abs() < EPS);
}

fn schema(update_rule: UpdateRule) -> AttributeSchema {
    AttributeSchema {
        bank_id: "b".into(),
        attr_key: "affinity".into(),
        update_rule,
        alpha: None,
        trend_window: None,
        bounds_lo: Some(0.0),
        bounds_hi: Some(1.0),
        clamp_values: true,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

#[test]
fn t01_schema_validation_and_knob_resolution() {
    schema(UpdateRule::Continuous).validate().unwrap();
    let mut s = schema(UpdateRule::Continuous);
    s.bank_id.clear();
    assert!(s.validate().is_err());
    s = schema(UpdateRule::Continuous);
    s.alpha = Some(1.5);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::OutOfRange);
    s.alpha = Some(0.0);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::OutOfRange);
    s.alpha = Some(0.2);
    s.trend_window = Some(0);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::OutOfRange);
    s.trend_window = Some(4);
    s.bounds_hi = None;
    assert!(s.validate().is_err());
    s.clamp_values = false;
    s.bounds_lo = None;
    s.validate().unwrap();
    // clamp without any bounds has nothing to clamp to.
    s.clamp_values = true;
    s.validate().unwrap_err();
    s.clamp_values = false;
    s.validate().unwrap();
    let cfg = ContinuousEngineConfig::from_schema(&s);
    assert!((cfg.alpha - 0.2).abs() < EPS);
    assert_eq!(cfg.trend_window, 4);
    assert_eq!(cfg.bounds, None);
    let bare = AttributeSchema {
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        ..schema(UpdateRule::Discrete)
    };
    let cfg = ContinuousEngineConfig::from_schema(&bare);
    assert!((cfg.alpha - DEFAULT_ALPHA).abs() < EPS);
    assert_eq!(cfg.trend_window, DEFAULT_TREND_WINDOW);
}

#[test]
fn schema_validation_rejects_bad_created_at_bounds_and_clamp() {
    let mut s = schema(UpdateRule::Continuous);
    s.created_at.clear();
    assert!(s.validate().is_err());
    s = schema(UpdateRule::Continuous);
    s.bounds_lo = Some(1.0);
    s.bounds_hi = Some(0.0);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::OutOfRange);
    s.bounds_lo = Some(f64::NAN);
    s.bounds_hi = Some(1.0);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::OutOfRange);
    s.bounds_lo = Some(0.0);
    s.bounds_hi = None;
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    s.clamp_values = true;
    s.bounds_lo = None;
    s.bounds_hi = Some(1.0);
    assert_eq!(s.validate().unwrap_err().code, ErrorCode::InvalidArgument);
}

#[test]
fn config_validation_rejects_bad_bounds() {
    let bad = ContinuousEngineConfig {
        bounds: Some(Bounds { lo: 2.0, hi: 1.0 }),
        ..cfg()
    };
    assert_eq!(bad.validate().unwrap_err().code, ErrorCode::OutOfRange);
    let nan = ContinuousEngineConfig {
        bounds: Some(Bounds {
            lo: f64::NAN,
            hi: 1.0,
        }),
        ..cfg()
    };
    assert_eq!(
        apply_observation(&nan, &[], 1.0, "t").unwrap_err().code,
        ErrorCode::OutOfRange
    );
}

/// (type level): `fact`/`belief` are not `update_rule` values.
#[test]
fn t13_epistemic_kind_is_not_update_rule() {
    for raw in ["fact", "belief", "class", ""] {
        let err = UpdateRule::parse(raw).unwrap_err();
        assert_eq!(err.code, ErrorCode::WrongUpdateRule, "{raw}");
        assert!(
            err.message
                .contains("update_rule must be discrete|continuous")
        );
    }
}
