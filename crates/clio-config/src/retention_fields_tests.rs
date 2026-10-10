// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the retention profile fields
//! added in profile version 3: the curve flag and floor, the pack fidelity
//! channel, the provenance floor, and the maintenance trigger.
//!
//! ## Owns
//! - Defaults read by a stored profile that omits the new fields
//! - Serialize/deserialize round trips and closed enum vocabularies
//! - Retention floor range validation and the version gate message
//! - Resolution of every new value onto `ResolvedRetention`
//!
//! ## Does not own
//! - The level enum wire mapping (see `retention_levels`)
//! - Admission scoring that consumes the resolved snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the new profile fields or their defaults should normally require
//! changes here. Changes to admission arithmetic should not.
//!
//! Keep this module focused on the version-3 profile fields.

use super::*;

#[test]
fn new_field_defaults_fill_a_payload_that_omits_them() {
    let old: RetentionProfile = serde_json::from_value(serde_json::json!({
        "verbosity": "selective",
        "version": 2
    }))
    .expect("a v2 payload without the new fields loads");
    assert!(old.retention_curve, "curve defaults on");
    assert!((old.retention_floor - DEFAULT_RETENTION_FLOOR).abs() < 1e-12);
    assert_eq!(old.pack_fidelity_default, PackFidelityDefault::Flow);
    assert_eq!(old.provenance_floor_default, None);
    assert!(old.maintenance_trigger, "maintenance trigger defaults on");
    assert_eq!(old.version, 2);
    // The omitted fields fill in, but the shared reader refuses the older marker:
    // only the stored path converts it before this check.
    assert_eq!(
        old.validate().expect_err("an older marker is refused").code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn profile_defaults_pin_the_documented_values() {
    let profile = RetentionProfile::default();
    assert!(profile.retention_curve);
    assert!((profile.retention_floor - 0.25).abs() < 1e-12);
    assert_eq!(profile.pack_fidelity_default, PackFidelityDefault::Flow);
    assert_eq!(profile.provenance_floor_default, None);
    assert!(profile.maintenance_trigger);
    assert_eq!(profile.version, 3);
}

#[test]
fn new_fields_round_trip_through_json() {
    let profile = RetentionProfile {
        retention_curve: false,
        retention_floor: 0.75,
        pack_fidelity_default: PackFidelityDefault::Exact,
        provenance_floor_default: Some(ProvenanceFloorDefault::ThirdParty),
        maintenance_trigger: false,
        ..RetentionProfile::default()
    };
    let text = serde_json::to_string(&profile).expect("serialize");
    let back: RetentionProfile = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, profile);
    let value: serde_json::Value = serde_json::from_str(&text).expect("value");
    assert_eq!(value["retention_curve"], false);
    assert_eq!(value["retention_floor"], 0.75);
    assert_eq!(value["pack_fidelity_default"], "exact");
    assert_eq!(value["provenance_floor_default"], "third_party");
    assert_eq!(value["maintenance_trigger"], false);
}

#[test]
fn a_none_provenance_floor_is_omitted_from_json() {
    let value = serde_json::to_value(RetentionProfile::default()).expect("value");
    assert!(value.get("provenance_floor_default").is_none());
}

#[test]
fn retention_floor_accepts_one_and_rejects_out_of_range() {
    for bad in [
        0.0_f64,
        -0.1,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1.5,
    ] {
        let profile = RetentionProfile {
            retention_floor: bad,
            ..RetentionProfile::default()
        };
        assert_eq!(
            profile.validate().expect_err("floor out of range").code,
            ErrorCode::OutOfRange,
            "floor {bad} must be refused"
        );
    }
    let one = RetentionProfile {
        retention_floor: 1.0,
        ..RetentionProfile::default()
    };
    one.validate().expect("floor 1 is the closed upper bound");
    let smallest = RetentionProfile {
        retention_floor: f64::MIN_POSITIVE,
        ..RetentionProfile::default()
    };
    smallest
        .validate()
        .expect("the smallest positive floor is inside (0, 1]");
}

#[test]
fn unknown_enum_values_are_refused_at_deserialization() {
    for payload in [
        serde_json::json!({"pack_fidelity_default": "lossless"}),
        serde_json::json!({"provenance_floor_default": "trusted"}),
    ] {
        let err = serde_json::from_value::<RetentionProfile>(payload).expect_err("unknown value");
        assert!(err.to_string().contains("unknown variant"), "{err}");
    }
}

/// The request-side refusal lives in `parse` and is not reachable through the
/// profile's own `Deserialize`. This is the wall the two paths depend on: the
/// lenient stored-floor fallback rewrites the raw file before anything is
/// typed, so the request surface, which parses a profile straight from the
/// caller's JSON, keeps refusing a mistyped rung instead of coercing it to
/// `verified`. Both paths read the same ladder, and only the stored one falls
/// back.
#[test]
fn the_request_path_refuses_a_mistyped_rung_while_a_stored_file_fails_closed() {
    // The request surface: a caller sends a profile directly.
    let refused = serde_json::from_value::<RetentionProfile>(
        serde_json::json!({"provenance_floor_default": "trusted"}),
    )
    .expect_err("a mistyped rung on a request is refused");
    assert!(
        refused.to_string().contains("unknown variant"),
        "the request path reports the refusal rather than storing a floor: {refused}"
    );

    // The stored surface: the same value in a file, resolved by the fallback.
    assert_eq!(
        fail_closed_stored_floor(&serde_json::json!("trusted")),
        Some(ProvenanceFloorDefault::Verified),
        "the stored path resolves the identical value to the strictest rung"
    );
}

#[test]
fn resolve_carries_every_new_profile_value() {
    let profile = RetentionProfile {
        retention_curve: false,
        retention_floor: 0.5,
        pack_fidelity_default: PackFidelityDefault::Exact,
        provenance_floor_default: Some(ProvenanceFloorDefault::Stated),
        maintenance_trigger: false,
        ..RetentionProfile::default()
    };
    let resolved = profile.resolve();
    assert!(!resolved.retention_curve());
    assert!((resolved.retention_floor() - 0.5).abs() < 1e-12);
    assert_eq!(resolved.pack_fidelity_default(), PackFidelityDefault::Exact);
    assert_eq!(
        resolved.provenance_floor_default(),
        Some(ProvenanceFloorDefault::Stated)
    );
    assert!(!resolved.maintenance_trigger());
}
