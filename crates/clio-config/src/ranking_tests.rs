// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the ranking-environment patch
//! surface: the full env, the optional patch, and the merge that validates it.
//!
//! ## Owns
//! - Tests for merging a patch onto a base environment
//! - Tests for the weight, coactivation, and range checks that merge applies
//!
//! ## Does not own
//! - The weight group shapes and their normalization (see `weight_groups`)
//! - The knob tables the validators read (see `admission_knobs`)
//!
//! ## Boundary
//!
//! Changes to the ranking-env schema or to patch validation should require a
//! change here. A change to a single weight group's fields should not.
//!
//! Keep this module focused on the ranking-environment patch surface.

use super::*;

#[test]
fn renormalize_retrieval_weights() {
    // Three weights of 1.0 add up to 3.0, so each becomes 1.0 / 3.0.
    let w = RetrievalWeights {
        dense: 1.0,
        lexical: 1.0,
        importance: 1.0,
    };
    let n = apply_retrieval(&w).expect("renorm");
    assert!((n.dense - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
fn reject_zero_sum_weights() {
    let err = normalize_weights(&[0.0, 0.0]).expect_err("zero");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn apply_retrieval_rejects_bad_weights() {
    let err = apply_retrieval(&RetrievalWeights {
        dense: 0.0,
        lexical: 0.0,
        importance: 0.0,
    })
    .expect_err("zero retrieval");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn reject_negative_weight() {
    let err = normalize_weights(&[0.5, -0.1]).expect_err("neg");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn preview_patch_rejects_bad_retrieval() {
    let base = RankingEnv::system_defaults();
    let err = preview_patch(
        &base,
        &RankingEnvPatch {
            retrieval: Some(RetrievalWeights {
                dense: -1.0,
                lexical: 0.0,
                importance: 0.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect_err("bad retrieval");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

/// A three-weight patch is rescaled by the merge, so the applied group sums to 1.
#[test]
fn preview_patch_renormalizes_three_retrieval_weights() {
    let base = RankingEnv::system_defaults();
    // 2.0 + 1.0 + 1.0 = 4.0, so the applied weights are 0.5, 0.25 and 0.25.
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            retrieval: Some(RetrievalWeights {
                dense: 2.0,
                lexical: 1.0,
                importance: 1.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("three weights apply");
    let applied = next.retrieval.as_slice();
    assert!((applied[0] - 0.5).abs() < 1e-12, "got {applied:?}");
    assert!((applied[1] - 0.25).abs() < 1e-12, "got {applied:?}");
    assert!((applied[2] - 0.25).abs() < 1e-12, "got {applied:?}");
    let sum: f64 = applied.iter().sum();
    assert!((sum - 1.0).abs() < 1e-12, "sum was {sum}");
}

#[test]
fn preview_patch_coactivation_and_theta() {
    let base = RankingEnv::system_defaults();
    let ok = preview_patch(
        &base,
        &RankingEnvPatch {
            coactivation: Some(CoactivationConstants::default()),
            intent_gate: Some(IntentGateThresholds {
                skip_confidence: Some(0.5),
            }),
            theta_admit: Some(0.6),
            ..RankingEnvPatch::default()
        },
    )
    .expect("ok");
    assert!((ok.theta_admit - 0.6).abs() < 1e-9);

    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                coactivation: Some(CoactivationConstants {
                    eta: 2.0,
                    ..CoactivationConstants::default()
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                coactivation: Some(CoactivationConstants {
                    half_life_days: 0.0,
                    ..CoactivationConstants::default()
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                coactivation: Some(CoactivationConstants {
                    w_min: 2.0,
                    ..CoactivationConstants::default()
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
}

#[test]
fn preview_patch_intent_and_admission_errors() {
    let base = RankingEnv::system_defaults();
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                intent_gate: Some(IntentGateThresholds {
                    skip_confidence: None,
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_ok()
    );
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                intent_gate: Some(IntentGateThresholds {
                    skip_confidence: Some(2.0),
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                theta_admit: Some(2.0),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
    assert!(
        preview_patch(
            &base,
            &RankingEnvPatch {
                admission: Some(AdmissionWeights {
                    w1: -1.0,
                    w2: 0.0,
                    w3: 0.0,
                    w4: 0.0,
                }),
                ..RankingEnvPatch::default()
            },
        )
        .is_err()
    );
}

#[test]
fn preview_patch_hygiene_weights_and_stale_half_life() {
    let base = RankingEnv::system_defaults();
    let ok = preview_patch(
        &base,
        &RankingEnvPatch {
            hygiene: Some(HygieneWeights {
                secret: 1.0,
                pattern: 1.0,
                dup: 1.0,
                utility: 1.0,
                staleness: 0.0,
            }),
            hygiene_stale_half_life_days: Some(30.0),
            ..RankingEnvPatch::default()
        },
    )
    .expect("ok hygiene patch");
    // renormalized to .8/.8/.8/.8/0 → .2/.2/.2/.2/0
    assert!((ok.hygiene.secret - 0.25).abs() < 1e-9);
    assert!((ok.hygiene_stale_half_life_days - 30.0).abs() < 1e-9);

    // negative/zero weights rejected
    let err = preview_patch(
        &base,
        &RankingEnvPatch {
            hygiene: Some(HygieneWeights {
                secret: -1.0,
                ..HygieneWeights::default()
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect_err("negative hygiene weight");
    assert_eq!(err.code, ErrorCode::OutOfRange);

    // zero-sum rejected
    let err = preview_patch(
        &base,
        &RankingEnvPatch {
            hygiene: Some(HygieneWeights {
                secret: 0.0,
                pattern: 0.0,
                dup: 0.0,
                utility: 0.0,
                staleness: 0.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);

    // stale half-life must be finite and positive
    for bad in [0.0, -1.0, f64::NAN] {
        let err = preview_patch(
            &base,
            &RankingEnvPatch {
                hygiene_stale_half_life_days: Some(bad),
                ..RankingEnvPatch::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::OutOfRange);
    }

    // The hygiene and retention defaults live on the system defaults; the
    // full-env JSON reader no longer exists, so they are checked in code.
    let defaults = RankingEnv::system_defaults();
    assert_eq!(defaults.hygiene, HygieneWeights::default());
    assert!((defaults.hygiene_stale_half_life_days - 90.0).abs() < 1e-9);
    assert_eq!(
        defaults.retention.half_life_days,
        DEFAULT_RETENTION_HALF_LIFE_DAYS
    );
}

/// A current patch with omitted nested values keeps the existing defaults and
/// produces the current output shape.
#[test]
fn patch_with_omitted_nested_values_keeps_defaults_and_output_shape() {
    let patch: RankingEnvPatch = serde_json::from_value(serde_json::json!({
        "retention": {},
        "trust": {},
        "maintenance_triggers": {}
    }))
    .expect("nested groups with omitted values are accepted");
    let base = RankingEnv::system_defaults();
    let next = preview_patch(&base, &patch).expect("defaults keep applying");
    assert_eq!(
        next.retention.half_life_days,
        DEFAULT_RETENTION_HALF_LIFE_DAYS
    );
    assert_eq!(next.trust, TrustRatios::default());
    assert_eq!(
        next.maintenance_triggers,
        MaintenanceTriggerKnobs::default()
    );
    let expected = serde_json::to_value(&base).expect("serialize base");
    let actual = serde_json::to_value(&next).expect("serialize patched env");
    assert_eq!(
        actual, expected,
        "omitted nested values keep the current output shape"
    );
}

#[test]
fn retention_knobs_default_to_thirty_days() {
    assert_eq!(DEFAULT_RETENTION_HALF_LIFE_DAYS, 30.0);
    assert_eq!(default_retention_half_life_days(), 30.0);
    assert_eq!(RetentionKnobs::default().half_life_days, 30.0);
    assert_eq!(RankingEnv::system_defaults().retention.half_life_days, 30.0);
}

#[test]
fn preview_patch_applies_a_retention_half_life() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            retention: Some(RetentionKnobs {
                half_life_days: 7.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("valid retention half-life");
    assert_eq!(next.retention.half_life_days, 7.0);
    assert_eq!(
        base.retention.half_life_days, 30.0,
        "base must not be mutated"
    );
}

/// Zero, negative, and non-finite retention half-lives are refused with the
/// range error, like every other decay constant.
#[test]
fn preview_patch_rejects_bad_retention_half_life() {
    let base = RankingEnv::system_defaults();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let err = preview_patch(
            &base,
            &RankingEnvPatch {
                retention: Some(RetentionKnobs {
                    half_life_days: bad,
                }),
                ..RankingEnvPatch::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::OutOfRange, "half-life {bad}");
    }
}

/// The nested group fills its own default and rejects unknown fields: the
/// outer `deny_unknown_fields` does not reach into a nested struct.
#[test]
fn retention_knobs_fill_defaults_and_reject_unknown_fields() {
    let filled: RetentionKnobs = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(filled.half_life_days, DEFAULT_RETENTION_HALF_LIFE_DAYS);

    let set: RetentionKnobs =
        serde_json::from_value(serde_json::json!({"half_life_days": 7.0})).unwrap();
    assert_eq!(set.half_life_days, 7.0);

    assert!(
        serde_json::from_value::<RetentionKnobs>(
            serde_json::json!({"half_life_days": 30.0, "unknown": 1})
        )
        .is_err()
    );
}

/// A ranking patch still rejects unknown fields at both levels.
#[test]
fn ranking_patch_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<RankingEnvPatch>(serde_json::json!({"unknown_group": {}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<RankingEnvPatch>(
            serde_json::json!({"retention": {"half_life_days": 30.0, "unknown": 1}})
        )
        .is_err()
    );
}

/// A saved environment that omits one group loads every value it does carry and
/// fills the omission from that group's own default, so a file written before a
/// knob existed is not discarded whole.
#[test]
fn a_saved_ranking_env_missing_a_group_keeps_the_values_it_carries() {
    let env: RankingEnv = serde_json::from_value(serde_json::json!({
        "retrieval": {"dense": 0.5, "lexical": 0.25, "importance": 0.25},
        "theta_admit": 0.61,
        "theta_overrides": {"note": 0.5},
    }))
    .expect("a missing group must not discard the environment");

    assert!((env.theta_admit - 0.61).abs() < 1e-9, "{env:?}");
    assert!((env.retrieval.dense - 0.5).abs() < 1e-9, "{env:?}");
    assert_eq!(
        env.theta_overrides,
        HashMap::from([("note".to_owned(), 0.5)])
    );
    assert_eq!(env.maintenance_triggers, MaintenanceTriggerKnobs::default());
    assert_eq!(env.trust, TrustRatios::default());
    assert_eq!(env.hygiene, HygieneWeights::default());
}

/// Every group omitted is the system default, so the defaults a load fills in
/// are the ones the code builds rather than the zeroed `Default` derive.
#[test]
fn an_empty_saved_ranking_env_loads_the_system_defaults() {
    let env: RankingEnv = serde_json::from_value(serde_json::json!({})).expect("all omitted");
    assert_eq!(env, RankingEnv::system_defaults());
}
