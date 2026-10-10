// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the published JSON Schema of
//! the retention profile's version-3 fields: types, defaults, bounds, and the
//! closed enum vocabularies.
//!
//! ## Owns
//! - Presence, type, default, and description of every new profile field
//! - The retention floor bounds and the closed pack/provenance vocabularies
//!
//! ## Does not own
//! - Schema `$ref` resolution (see `retention`'s schema tests)
//! - Profile validation itself (see `retention`)
//!
//! ## Boundary
//!
//! Changes to the published schema of the new fields should normally require
//! changes here. Changes to mapping tables should not.
//!
//! Keep this module focused on the schema's version-3 fields.

use super::*;
use crate::retention::DEFAULT_RETENTION_FLOOR;

#[test]
fn schema_names_every_new_field_with_a_default_and_description() {
    let schema = profile_json_schema();
    let props = &schema["properties"];
    assert_eq!(props["retention_curve"]["type"], "boolean");
    assert_eq!(props["retention_curve"]["default"], true);
    assert_eq!(props["retention_floor"]["type"], "number");
    assert_eq!(props["retention_floor"]["default"], DEFAULT_RETENTION_FLOOR);
    assert_eq!(props["pack_fidelity_default"]["type"], "string");
    assert_eq!(props["pack_fidelity_default"]["default"], "flow");
    assert_eq!(
        props["provenance_floor_default"]["type"],
        serde_json::json!(["string", "null"])
    );
    assert!(
        props["provenance_floor_default"]
            .get("default")
            .is_some_and(serde_json::Value::is_null),
        "the provenance floor default must be an explicit null: {schema}"
    );
    assert_eq!(props["maintenance_trigger"]["type"], "boolean");
    assert_eq!(props["maintenance_trigger"]["default"], true);
    for key in [
        "retention_curve",
        "retention_floor",
        "pack_fidelity_default",
        "provenance_floor_default",
        "maintenance_trigger",
    ] {
        assert!(
            props[key]["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "{key} needs a description: {schema}"
        );
    }
}

#[test]
fn schema_bounds_and_vocabularies_are_closed() {
    let schema = profile_json_schema();
    let floor = &schema["properties"]["retention_floor"];
    assert_eq!(floor["exclusiveMinimum"], 0.0);
    assert_eq!(floor["maximum"], 1.0);
    assert_eq!(
        schema["properties"]["pack_fidelity_default"]["enum"],
        serde_json::json!(["flow", "exact"])
    );
    assert_eq!(
        schema["properties"]["provenance_floor_default"]["enum"],
        serde_json::json!(["verified", "stated", "third_party", "inferred", null])
    );
    assert_eq!(schema["x-retention-profile-version"], 3);
}
