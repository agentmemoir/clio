// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the retention tool schemas:
//! published defs, mapping tables, and pack wiring.

use super::*;
use crate::schema::{catalog_defs, schema_pack, validate_tool_schema};
use serde_json::Value;

fn def(name: &str) -> ToolDef {
    retention_defs()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn retention_defs_are_structurally_valid() {
    let defs = retention_defs();
    let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
    assert_eq!(
        names,
        vec![
            "admit_preview_batch",
            "retention_profile_get",
            "retention_profile_set"
        ]
    );
    for d in &defs {
        let defects = validate_tool_schema(&d.to_tool());
        assert!(defects.is_empty(), "{defects:?}");
    }
}

#[test]
fn batch_preview_schema_caps_items_and_reuses_candidate_fields() {
    let tool = def("admit_preview_batch").to_tool();
    assert_eq!(
        tool["inputSchema"]["required"],
        serde_json::json!(["items"])
    );
    let item = &tool["inputSchema"]["properties"]["items"]["items"];
    assert_eq!(item["type"], "object");
    // Candidate fields come from the single-item preview schema, minus bank/actor.
    assert!(item["properties"]["category"]["enum"].is_array());
    assert!(item["properties"]["episodic_type"]["enum"].is_array());
    assert!(item["properties"]["epistemic_kind"]["enum"].is_array());
    assert!(item["properties"].get("bank").is_none());
    assert!(item["properties"].get("actor").is_none());
    let override_props = &tool["inputSchema"]["properties"]["profile_override"]["properties"];
    assert!(override_props["verbosity"]["enum"].is_array());
    assert_eq!(override_props["verbosity"]["enum"][0], "selective");
}

#[test]
fn profile_set_schema_publishes_the_mapping_tables_and_confirmation() {
    let tool = def("retention_profile_set").to_tool();
    assert_eq!(
        tool["inputSchema"]["required"],
        serde_json::json!(["profile"])
    );
    let profile = &tool["inputSchema"]["properties"]["profile"]["properties"];
    assert_eq!(profile["duplicate_tolerance_write"]["enum"][0], "strict");
    assert_eq!(
        profile["recall_scope_default"]["enum"][1],
        "consolidated_only"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["scope"]["enum"][1],
        "deployment"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["confirm"]["type"],
        "boolean"
    );
    let profile_def = &tool["inputSchema"]["properties"]["profile"];
    assert_eq!(profile_def["additionalProperties"], false);
}

#[test]
fn profile_set_schema_publishes_the_retention_knob_fields() {
    let tool = def("retention_profile_set").to_tool();
    let profile = &tool["inputSchema"]["properties"]["profile"]["properties"];
    assert_eq!(profile["retention_curve"]["type"], "boolean");
    assert_eq!(profile["retention_curve"]["default"], true);
    assert_eq!(profile["retention_floor"]["type"], "number");
    assert_eq!(profile["retention_floor"]["default"], 0.25);
    assert_eq!(profile["retention_floor"]["exclusiveMinimum"], 0.0);
    assert_eq!(profile["retention_floor"]["maximum"], 1.0);
    assert_eq!(
        profile["pack_fidelity_default"]["enum"],
        serde_json::json!(["flow", "exact"])
    );
    assert_eq!(profile["pack_fidelity_default"]["default"], "flow");
    assert_eq!(
        profile["provenance_floor_default"]["default"],
        serde_json::Value::Null
    );
    assert!(
        profile["provenance_floor_default"]["enum"]
            .as_array()
            .is_some()
    );
    assert_eq!(profile["maintenance_trigger"]["type"], "boolean");
    assert_eq!(profile["maintenance_trigger"]["default"], true);
    // The batch override embeds the same published profile schema.
    let override_props = &def("admit_preview_batch").to_tool()["inputSchema"]["properties"]["profile_override"]
        ["properties"];
    assert_eq!(override_props["retention_floor"]["default"], 0.25);
}

#[test]
fn profile_set_schema_publishes_mission_fields() {
    let tool = def("retention_profile_set").to_tool();
    let profile = &tool["inputSchema"]["properties"]["profile"]["properties"];
    assert_eq!(profile["mission"]["type"], "object");
    assert_eq!(profile["mission"]["additionalProperties"], false);
    assert_eq!(
        profile["mission"]["required"],
        serde_json::json!(["mission_statement", "drop_examples"])
    );
    assert_eq!(
        profile["mission"]["properties"]["mission_statement"]["type"],
        "string"
    );
    assert_eq!(
        profile["mission"]["properties"]["drop_examples"]["minItems"],
        1
    );
    // The batch override embeds the same published profile schema.
    let override_props = &def("admit_preview_batch").to_tool()["inputSchema"]["properties"]["profile_override"]
        ["properties"];
    assert_eq!(override_props["mission"]["type"], "object");
    let defects = validate_tool_schema(&def("retention_profile_set").to_tool());
    assert!(defects.is_empty(), "{defects:?}");
}

#[test]
fn published_pack_carries_mission_fields_and_rule_ids() {
    let pack = serde_json::to_string(&schema_pack()).expect("serialize");
    assert!(pack.contains("\"mission\""), "mission schema missing");
    assert!(
        pack.contains("mission_rule_ids"),
        "mission_rule_ids missing from the pack"
    );
}

#[test]
fn retention_tools_are_in_the_published_pack_with_the_pinned_revision() {
    let pack = schema_pack();
    assert_eq!(pack["mcp_protocol_revision"], "2025-11-25");
    let names: Vec<&str> = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "admit_preview_batch",
        "retention_profile_get",
        "retention_profile_set",
    ] {
        assert!(names.contains(&name), "`{name}` missing from the pack");
    }
    // The pack stays free of a `title` key that could collide with category `schema`.
    let raw = serde_json::to_string(&pack).expect("serialize");
    assert!(!raw.contains("\"title\""));
    // `catalog_defs` is the single source of truth for the pack.
    let published: Vec<&str> = catalog_defs().iter().map(|d| d.name).collect();
    for name in names {
        assert!(published.contains(&name), "`{name}` not from catalog_defs");
    }
    let _: &Value = &pack;
}
