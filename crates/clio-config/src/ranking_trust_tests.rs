// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the trust-ratio group on the
//! ranking-environment surface: its defaults, its serde shape, its bounds, and
//! how the patch merge applies it.
//!
//! ## Owns
//! - Tests for the trust group's defaults and per-field serde defaults
//! - Tests for trust-ratio bounds in the patch merge and the serialized env
//!
//! ## Does not own
//! - The rest of the ranking-env patch surface (see `ranking_tests`)
//! - Applying the ratios when scoring memories
//!
//! ## Boundary
//!
//! Changes to the trust group's fields or bounds should require a change here.
//! Changes to other ranking knobs should not.
//!
//! Keep this module focused on the trust-ratio group.

use super::*;

/// Every layer that names a trust default agrees on the documented value:
/// the constant, its serde default function, the struct default, and the
/// system defaults.
#[test]
fn trust_ratios_default_to_documented_values() {
    assert_eq!(DEFAULT_TRUST_USER_STATED, 1.0, "user_stated constant");
    assert_eq!(DEFAULT_TRUST_THIRD_PARTY, 0.95, "third_party constant");
    assert_eq!(
        DEFAULT_TRUST_AGENT_INFERRED, 0.85,
        "agent_inferred constant"
    );
    assert_eq!(
        default_trust_user_stated(),
        DEFAULT_TRUST_USER_STATED,
        "user_stated serde default"
    );
    assert_eq!(
        default_trust_third_party(),
        DEFAULT_TRUST_THIRD_PARTY,
        "third_party serde default"
    );
    assert_eq!(
        default_trust_agent_inferred(),
        DEFAULT_TRUST_AGENT_INFERRED,
        "agent_inferred serde default"
    );
    let defaults = TrustRatios::default();
    assert_eq!(defaults.user_stated, DEFAULT_TRUST_USER_STATED);
    assert_eq!(defaults.third_party, DEFAULT_TRUST_THIRD_PARTY);
    assert_eq!(defaults.agent_inferred, DEFAULT_TRUST_AGENT_INFERRED);
    assert_eq!(
        RankingEnv::system_defaults().trust,
        defaults,
        "system defaults carry the struct defaults"
    );
}

/// A stored trust group holds fields from before every field existed, so each
/// missing field must load at its own serde default while an explicit field
/// still wins.
#[test]
fn trust_ratios_fill_per_field_serde_defaults() {
    let filled: TrustRatios = serde_json::from_value(serde_json::json!({})).expect("empty object");
    assert_eq!(
        filled,
        TrustRatios::default(),
        "an empty object fills every field"
    );

    let partial: TrustRatios =
        serde_json::from_value(serde_json::json!({"third_party": 0.5})).expect("partial object");
    assert_eq!(
        partial.user_stated, DEFAULT_TRUST_USER_STATED,
        "user_stated keeps its default"
    );
    assert_eq!(partial.third_party, 0.5, "the explicit field wins");
    assert_eq!(
        partial.agent_inferred, DEFAULT_TRUST_AGENT_INFERRED,
        "agent_inferred keeps its default"
    );
}

/// The trust group is closed: a field outside its three sources is refused,
/// not silently dropped.
#[test]
fn trust_ratios_reject_unknown_fields() {
    assert!(
        serde_json::from_value::<TrustRatios>(
            serde_json::json!({"user_stated": 1.0, "unknown": 1})
        )
        .is_err(),
        "an unknown field must be refused, not ignored"
    );
}

/// The patch merge copies each ratio onto the base and leaves the base alone.
#[test]
fn preview_patch_applies_trust_ratios() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            trust: Some(TrustRatios {
                user_stated: 1.0,
                third_party: 0.5,
                agent_inferred: 0.25,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("valid trust ratios");
    assert_eq!(next.trust.user_stated, 1.0);
    assert_eq!(next.trust.third_party, 0.5);
    assert_eq!(next.trust.agent_inferred, 0.25);
    assert_eq!(
        base.trust,
        TrustRatios::default(),
        "base must not be mutated"
    );
}

/// `ranking_env_get` serializes the env, so the trust group must appear in the
/// payload, one key per field.
#[test]
fn ranking_env_serializes_trust_ratios() {
    let env = RankingEnv {
        trust: TrustRatios {
            user_stated: 1.0,
            third_party: 0.5,
            agent_inferred: 0.25,
        },
        ..RankingEnv::system_defaults()
    };
    let json = serde_json::to_value(&env).expect("serialize ranking env");
    assert_eq!(json["trust"]["user_stated"], serde_json::json!(1.0));
    assert_eq!(json["trust"]["third_party"], serde_json::json!(0.5));
    assert_eq!(json["trust"]["agent_inferred"], serde_json::json!(0.25));
}

/// Every ratio must be finite and inside `(0, 1]`; zero, negatives,
/// non-finite values, and values above the upper bound are all refused with
/// the range error, per field.
#[test]
fn preview_patch_rejects_bad_trust_ratios() {
    let base = RankingEnv::system_defaults();
    for bad in [0.0, -1.0, 1.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for (name, trust) in [
            (
                "user_stated",
                TrustRatios {
                    user_stated: bad,
                    ..TrustRatios::default()
                },
            ),
            (
                "third_party",
                TrustRatios {
                    third_party: bad,
                    ..TrustRatios::default()
                },
            ),
            (
                "agent_inferred",
                TrustRatios {
                    agent_inferred: bad,
                    ..TrustRatios::default()
                },
            ),
        ] {
            let err = preview_patch(
                &base,
                &RankingEnvPatch {
                    trust: Some(trust),
                    ..RankingEnvPatch::default()
                },
            )
            .unwrap_err();
            assert_eq!(err.code, ErrorCode::OutOfRange, "{name} = {bad}");
        }
    }
}

/// The upper bound itself is allowed: `1.0` passes for every field.
#[test]
fn preview_patch_accepts_trust_ratio_upper_bound() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            trust: Some(TrustRatios {
                user_stated: 1.0,
                third_party: 1.0,
                agent_inferred: 1.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("1.0 is inside (0, 1]");
    assert_eq!(next.trust.user_stated, 1.0);
    assert_eq!(next.trust.third_party, 1.0);
    assert_eq!(next.trust.agent_inferred, 1.0);
}

/// A patch whose trust group omits every field fills the group's defaults,
/// so an env updated through such a patch keeps the documented ratios.
#[test]
fn stored_ranking_env_defaults_trust_when_absent() {
    let patch: RankingEnvPatch =
        serde_json::from_value(serde_json::json!({"trust": {}})).expect("omitted trust group");
    let next = preview_patch(&RankingEnv::system_defaults(), &patch).expect("patch applies");
    assert_eq!(
        next.trust,
        TrustRatios::default(),
        "an omitted trust group fills the defaults"
    );
}

/// A trust group sent through the patch's JSON shape must deserialize into
/// the typed group, field by field.
#[test]
fn ranking_patch_parses_trust_group() {
    let patch: RankingEnvPatch = serde_json::from_value(serde_json::json!({
        "trust": {"user_stated": 1.0, "third_party": 0.5, "agent_inferred": 0.25}
    }))
    .expect("trust group accepted");
    let trust = patch.trust.expect("trust present");
    assert_eq!(trust.user_stated, 1.0);
    assert_eq!(trust.third_party, 0.5);
    assert_eq!(trust.agent_inferred, 0.25);
}

/// The patch's `deny_unknown_fields` reaches inside the nested trust group,
/// and still refuses an unknown top-level field.
#[test]
fn ranking_patch_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
            "trust": {"user_stated": 1.0, "unknown": 1}
        }))
        .is_err(),
        "an unknown field inside the trust group must be refused"
    );
    assert!(
        serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
            "trust": {"user_stated": 1.0},
            "bypass": true
        }))
        .is_err(),
        "an unknown top-level field must be refused"
    );
}
