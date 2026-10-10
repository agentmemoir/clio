// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the retention profile object itself:
//! level parsing and mapping, validation, resolution to a snapshot, and the
//! published profile schema.
//!
//! ## Owns
//! - Parsing, string round-trip, and mapping tests for verbosity, duplicate
//!   tolerance, and recall scope
//! - Validation tests for the version gate, mission rules, and theta offsets
//! - Resolution tests for the effective snapshot and thresholds
//! - Tests for the accepted-but-inert field envelope
//! - Tests for the published profile schema's shape and references
//!
//! ## Does not own
//! - The profile type and the schema builder (see `retention` and
//!   `retention_schema`)
//! - Bank-scoped profile storage (see `retention_store`)
//! - Stored marker conversion and stored floor behavior (see
//!   `retention_store_version_tests` and `retention_store_floor_tests`)
//!
//! ## Boundary
//!
//! A change to how a profile parses, validates, or resolves should normally
//! require changes here. A change to storage or to the schema builder's
//! internals should not.
//!
//! Keep this module focused on the profile object's own contract.

use super::*;
use crate::retention_schema::profile_json_schema;

fn profile_with(verbosity: Verbosity, tolerance: DuplicateTolerance) -> RetentionProfile {
    RetentionProfile {
        verbosity,
        duplicate_tolerance_write: tolerance,
        ..RetentionProfile::default()
    }
}

#[test]
fn verbosity_levels_parse_and_map() {
    assert_eq!(
        Verbosity::parse("selective").expect("s"),
        Verbosity::Selective
    );
    assert_eq!(
        Verbosity::parse("balanced").expect("b"),
        Verbosity::Balanced
    );
    assert_eq!(
        Verbosity::parse("permissive").expect("p"),
        Verbosity::Permissive
    );
    assert_eq!(
        Verbosity::parse("verbose").expect_err("renamed").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(Verbosity::Selective.as_str(), "selective");
    assert_eq!(Verbosity::Balanced.as_str(), "balanced");
    assert_eq!(Verbosity::Permissive.as_str(), "permissive");
    assert!((Verbosity::Selective.theta_delta() - SELECTIVE_THETA_DELTA).abs() < 1e-12);
    assert!((Verbosity::Permissive.theta_delta() - PERMISSIVE_THETA_DELTA).abs() < 1e-12);
    assert!(Verbosity::Balanced.theta_delta().abs() < 1e-12);
    assert!((Verbosity::Selective.novelty_floor() - SELECTIVE_NOVELTY_FLOOR).abs() < 1e-12);
    assert!(Verbosity::Balanced.novelty_floor().abs() < 1e-12);
    assert!(Verbosity::Permissive.novelty_floor().abs() < 1e-12);
    assert_eq!(Verbosity::default(), Verbosity::Balanced);
}

#[test]
fn duplicate_tolerance_levels_parse_and_map() {
    assert_eq!(
        DuplicateTolerance::parse("strict").expect("strict"),
        DuplicateTolerance::Strict
    );
    assert_eq!(
        DuplicateTolerance::parse("balanced").expect("balanced"),
        DuplicateTolerance::Balanced
    );
    assert_eq!(
        DuplicateTolerance::parse("lenient").expect("lenient"),
        DuplicateTolerance::Lenient
    );
    assert_eq!(
        DuplicateTolerance::parse("nope").expect_err("bad").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(DuplicateTolerance::Strict.as_str(), "strict");
    assert_eq!(DuplicateTolerance::Balanced.as_str(), "balanced");
    assert_eq!(DuplicateTolerance::Lenient.as_str(), "lenient");
    assert!(
        (DuplicateTolerance::Strict.write_novelty_floor() - STRICT_WRITE_NOVELTY_FLOOR).abs()
            < 1e-12
    );
    assert!(DuplicateTolerance::Balanced.write_novelty_floor().abs() < 1e-12);
    assert!(DuplicateTolerance::Lenient.write_novelty_floor().abs() < 1e-12);
    assert_eq!(RecallScope::Full.as_str(), "full");
    assert_eq!(RecallScope::ConsolidatedOnly.as_str(), "consolidated_only");
}

#[test]
fn validate_rejects_bad_version_keys_and_offsets() {
    RetentionProfile::default()
        .validate()
        .expect("default valid");

    let bad_version = RetentionProfile {
        version: 4,
        ..RetentionProfile::default()
    };
    let version_error = bad_version.validate().expect_err("version");
    assert_eq!(version_error.code, ErrorCode::OutOfRange);
    assert!(
        version_error
            .to_string()
            .contains(&format!("expected {RETENTION_PROFILE_VERSION}")),
        "{version_error}"
    );

    // The shared reader accepts only the current version. A stored file with an
    // older marker is converted before it is typed, so a request that carried
    // one reaches this check as written and is refused.
    for version in [1_u32, 2] {
        let stale = RetentionProfile {
            version,
            ..RetentionProfile::default()
        };
        let stale_error = stale.validate().expect_err("older marker");
        assert_eq!(stale_error.code, ErrorCode::OutOfRange);
        assert!(
            stale_error
                .to_string()
                .contains(&format!("expected {RETENTION_PROFILE_VERSION}")),
            "{stale_error}"
        );
    }

    // A mission is read on the current version only: an older marker is refused
    // by the version gate before the mission is considered.
    let mission = Some(crate::retention_mission::RetentionMission {
        mission_statement: "s".to_owned(),
        keep_examples: vec![],
        drop_examples: vec![crate::retention_mission::MissionEntry {
            text: "tests passed".to_owned(),
            match_kind: crate::retention_mission::MissionMatchKind::Literal,
            reject: true,
        }],
    });
    let with_mission = RetentionProfile {
        mission: mission.clone(),
        ..RetentionProfile::default()
    };
    with_mission
        .validate()
        .expect("a mission is valid on the current version");
    for version in [1_u32, 2] {
        let stale = RetentionProfile {
            mission: mission.clone(),
            version,
            ..RetentionProfile::default()
        };
        assert_eq!(
            stale
                .validate()
                .expect_err("mission on an older marker")
                .code,
            ErrorCode::OutOfRange
        );
    }

    let unknown_key = RetentionProfile {
        theta_offsets: BTreeMap::from([("not_a_key".to_owned(), 0.1)]),
        ..RetentionProfile::default()
    };
    assert_eq!(
        unknown_key.validate().expect_err("key").code,
        ErrorCode::UnknownPath
    );

    for bad in [0.5_f64, -0.5, f64::NAN, f64::INFINITY] {
        let p = RetentionProfile {
            theta_offsets: BTreeMap::from([("persona".to_owned(), bad)]),
            ..RetentionProfile::default()
        };
        assert_eq!(
            p.validate().expect_err("offset").code,
            ErrorCode::OutOfRange
        );
    }

    let bounded = RetentionProfile {
        theta_offsets: BTreeMap::from([("persona".to_owned(), MAX_THETA_OFFSET)]),
        ..RetentionProfile::default()
    };
    bounded.validate().expect("boundary allowed");
}

#[test]
fn resolve_builds_snapshot_and_effective_threshold() {
    let builtin = ResolvedRetention::builtin_default();
    assert_eq!(builtin, ResolvedRetention::default());
    assert!(builtin.theta_delta().abs() < 1e-12);
    assert!(builtin.novelty_floor().abs() < 1e-12);
    assert_eq!(builtin.profile_version(), RETENTION_PROFILE_VERSION);
    assert!(builtin.theta_delta_for("persona").abs() < 1e-12);

    let mut profile = profile_with(Verbosity::Selective, DuplicateTolerance::Strict);
    profile.theta_offsets.insert("persona".to_owned(), 0.10);
    profile.theta_offsets.insert("unknown_key".to_owned(), 0.10);
    let resolved = profile.resolve();
    assert!((resolved.theta_delta() - SELECTIVE_THETA_DELTA).abs() < 1e-12);
    // strict floor (0.25) wins over the selective floor (0.15).
    assert!((resolved.novelty_floor() - STRICT_WRITE_NOVELTY_FLOOR).abs() < 1e-12);
    assert!((resolved.theta_delta_for("persona") - (SELECTIVE_THETA_DELTA + 0.10)).abs() < 1e-12);
    // Unknown keys are ignored in the resolved snapshot (validation rejects them earlier).
    assert!((resolved.theta_delta_for("unknown_key") - SELECTIVE_THETA_DELTA).abs() < 1e-12);
    assert!((resolved.effective_threshold(0.55, "persona") - 0.73).abs() < 1e-9);
    // Clamped into [0, 1].
    assert!((resolved.effective_threshold(0.99, "persona") - 1.0).abs() < 1e-12);

    let permissive = profile_with(Verbosity::Permissive, DuplicateTolerance::Lenient).resolve();
    assert!((permissive.effective_threshold(0.02, "persona") - 0.0).abs() < 1e-12);
    assert!(permissive.novelty_floor().abs() < 1e-12);

    // Selective + balanced tolerance keeps the verbosity floor.
    let selective = profile_with(Verbosity::Selective, DuplicateTolerance::Balanced).resolve();
    assert!((selective.novelty_floor() - SELECTIVE_NOVELTY_FLOOR).abs() < 1e-12);
}

#[test]
fn accepted_but_inert_envelope_names_the_honoring_capability() {
    let profile = RetentionProfile {
        duplicate_tolerance_read: DuplicateTolerance::Strict,
        recall_scope_default: RecallScope::ConsolidatedOnly,
        ..RetentionProfile::default()
    };
    let policy = profile.accepted_but_inert();
    assert_eq!(policy["duplicate_tolerance_read"], "strict");
    assert_eq!(policy["recall_scope_default"], "consolidated_only");
    assert_eq!(policy["honored_by"], INERT_FIELDS_HONORED_BY);
    assert_eq!(policy["status"], "active");
    assert!(policy["note"].as_str().expect("note").contains("active"));
}

#[test]
fn read_tolerance_caps_and_scope_parse() {
    assert!(
        (DuplicateTolerance::Strict.read_similarity_cap() - STRICT_READ_SIMILARITY).abs() < 1e-12
    );
    assert!(
        (DuplicateTolerance::Balanced.read_similarity_cap() - BALANCED_READ_SIMILARITY).abs()
            < 1e-12
    );
    assert!(
        (DuplicateTolerance::Lenient.read_similarity_cap() - LENIENT_READ_SIMILARITY).abs() < 1e-12
    );
    // The read cap is independent of the write floor.
    assert!(DuplicateTolerance::Balanced.write_novelty_floor().abs() < 1e-12);
    assert!(DuplicateTolerance::Balanced.read_similarity_cap() > 0.0);

    assert_eq!(RecallScope::parse("full").expect("full"), RecallScope::Full);
    assert_eq!(
        RecallScope::parse("consolidated_only").expect("only"),
        RecallScope::ConsolidatedOnly
    );
    assert_eq!(
        RecallScope::parse("observations")
            .expect_err("unknown")
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(RecallScope::default(), RecallScope::Full);
}

#[test]
fn published_profile_schema_carries_no_phase_number() {
    let schema = profile_json_schema().to_string();
    assert!(!schema.contains("honored_from_phase"), "{schema}");
    assert!(
        !schema.to_ascii_lowercase().contains("phase 0"),
        "profile schema must not name a roadmap phase: {schema}"
    );
    assert_eq!(
        profile_json_schema()["x-inert-fields-honored-by"],
        INERT_FIELDS_HONORED_BY
    );
}

#[test]
fn serde_defaults_fill_missing_fields_and_reject_unknown() {
    let env: RetentionProfile = serde_json::from_value(serde_json::json!({})).expect("empty");
    assert_eq!(env, RetentionProfile::default());

    let partial: RetentionProfile =
        serde_json::from_value(serde_json::json!({"verbosity": "selective"})).expect("partial");
    assert_eq!(partial.verbosity, Verbosity::Selective);
    assert_eq!(partial.version, RETENTION_PROFILE_VERSION);

    let err = serde_json::from_value::<RetentionProfile>(
        serde_json::json!({"verbosity": "selective", "extra": true}),
    )
    .expect_err("unknown field");
    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn published_schema_carries_version_and_exact_mappings() {
    let schema = profile_json_schema();
    assert_eq!(
        schema["x-retention-profile-version"],
        RETENTION_PROFILE_VERSION
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["x-verbosity-mapping"]["selective"]["theta_delta"],
        SELECTIVE_THETA_DELTA
    );
    assert_eq!(
        schema["x-verbosity-mapping"]["permissive"]["theta_delta"],
        PERMISSIVE_THETA_DELTA
    );
    assert_eq!(
        schema["x-duplicate-tolerance-write-mapping"]["strict"]["novelty_floor"],
        STRICT_WRITE_NOVELTY_FLOOR
    );
    assert_eq!(
        schema["x-duplicate-tolerance-read-mapping"]["strict"]["similarity_cap"],
        STRICT_READ_SIMILARITY
    );
    assert_eq!(
        schema["x-duplicate-tolerance-read-mapping"]["balanced"]["similarity_cap"],
        BALANCED_READ_SIMILARITY
    );
    assert_eq!(
        schema["x-duplicate-tolerance-read-mapping"]["lenient"]["similarity_cap"],
        LENIENT_READ_SIMILARITY
    );
    assert_eq!(schema["x-theta-offset-bound"], MAX_THETA_OFFSET);
    let keys = schema["properties"]["theta_offsets"]["propertyNames"]["enum"]
        .as_array()
        .expect("key enum");
    assert_eq!(keys.len(), KNOWN_ADMISSION_KEYS.len());
    assert_eq!(keys.len(), ADMISSION_KEY_COUNT);
    assert!(
        keys.iter().any(|k| k.as_str() == Some("belief")),
        "belief create key must be tunable"
    );
    // Vocabulary guard: never a `title` that could collide with category `schema`.
    assert!(
        !serde_json::to_string(&schema)
            .expect("json")
            .contains("\"title\"")
    );
    assert_eq!(key_index("persona"), Some(4));
    assert_eq!(key_index("nope"), None);
}

/// The published profile schema must never carry a dangling `$ref`: pointers
/// are document-root-relative, so the mission entry schema is inlined instead
/// of referenced. Also self-checks the pointer-resolution walker it relies on.
#[test]
fn published_schema_has_no_dangling_refs() {
    use crate::retention_schema::schema_refs_resolve;

    let schema = profile_json_schema();
    assert!(
        schema_refs_resolve(&schema),
        "published profile schema has an unresolvable $ref: {schema}"
    );
    let mission = &schema["properties"]["mission"];
    for list in ["keep_examples", "drop_examples"] {
        let items = &mission["properties"][list]["items"];
        assert!(
            items.get("properties").is_some(),
            "entry schema is inlined: {items}"
        );
        assert!(
            items.get("$ref").is_none(),
            "no $ref under embedding: {items}"
        );
    }

    let dangling = serde_json::json!({"properties": {"a": {"$ref": "#/definitions/missing"}}});
    assert!(!schema_refs_resolve(&dangling));
    let resolving = serde_json::json!({"definitions": {"e": {"type": "string"}}, "a": {"$ref": "#/definitions/e"}});
    assert!(schema_refs_resolve(&resolving));
    let escaped = serde_json::json!({"x/a~b": {"type": "string"}, "a": {"$ref": "#/x~1a~0b"}});
    assert!(schema_refs_resolve(&escaped));
    let array = serde_json::json!({"defs": [{"type": "string"}], "a": {"$ref": "#/defs/0"}});
    assert!(schema_refs_resolve(&array));
    let bad_array = serde_json::json!({"defs": [{"type": "string"}], "a": {"$ref": "#/defs/2"}});
    assert!(!schema_refs_resolve(&bad_array));
}
