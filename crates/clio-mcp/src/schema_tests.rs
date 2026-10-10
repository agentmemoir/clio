// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Schema pack tests: (pack validates), (enum split)
//! (category `schema` value vs `tool_schema` vocabulary), write-set presence.

use super::*;
use crate::{bound_tools, bound_write_tools};
use serde_json::Value;

fn pack() -> Value {
    schema_pack()
}

#[test]
fn t01_pack_contains_all_core_catalog_tools_with_valid_schemas() {
    let pack = pack();
    assert_eq!(pack["format"], PACK_FORMAT);
    assert_eq!(pack["mcp_protocol_revision"], MCP_PROTOCOL_REVISION);
    let tools = pack["tools"].as_array().expect("tools array");
    for tool in tools {
        let defects = validate_tool_schema(tool);
        assert!(defects.is_empty(), "{defects:?}");
    }
    // The pack publishes exactly the bound tool set (Core + additive +
    // portability + config/ranking). Assert the exact published name
    // set, so both a missing and an extra tool fail the test — not merely
    // presence.
    let mut published: Vec<&str> = catalog_defs().iter().map(|d| d.name).collect();
    published.sort_unstable();
    let mut expected: Vec<&str> = bound_tools();
    expected.sort_unstable();
    assert_eq!(published, expected);
}

#[test]
fn t02_epistemic_kind_and_update_rule_are_separate_enums() {
    let pack = pack();
    let find = |name: &str| {
        pack["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} missing"))
            .clone()
    };
    let update = find("update");
    let rule = &update["inputSchema"]["properties"]["update_rule"]["enum"];
    assert_eq!(rule, &serde_json::json!(["discrete", "continuous"]));
    let store = find("store");
    let kind = &store["inputSchema"]["properties"]["epistemic_kind"]["enum"];
    assert_eq!(kind, &serde_json::json!(["fact", "belief"]));
    // Enum values stay separate: update never accepts fact/belief as a rule.
    assert!(
        !rule
            .as_array()
            .expect("enum")
            .contains(&Value::from("fact"))
    );
    assert!(
        !rule
            .as_array()
            .expect("enum")
            .contains(&Value::from("belief"))
    );
}

#[test]
fn t14_category_schema_appears_only_as_enum_value() {
    let pack = pack();
    let raw = serde_json::to_string(&pack).expect("serialize");
    assert!(
        !raw.contains("\"title\""),
        "pack must not carry a title that could collide with category `schema`"
    );
    let store = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"].as_str() == Some("store"))
        .expect("store")
        .clone();
    let categories = store["inputSchema"]["properties"]["category"]["enum"]
        .as_array()
        .expect("category enum");
    assert!(categories.iter().any(|v| v.as_str() == Some("schema")));
}

#[test]
fn every_bound_write_tool_is_in_the_pack() {
    let pack = pack();
    let names: Vec<&str> = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for bound in bound_write_tools() {
        assert!(
            names.contains(bound),
            "bound tool `{bound}` missing from pack"
        );
    }
}

#[test]
fn catalog_hash_is_deterministic_and_covers_tools() {
    let a = pack();
    let b = pack();
    assert_eq!(a["catalog_hash"], b["catalog_hash"]);
    let empty = catalog_hash(&[]);
    assert_ne!(a["catalog_hash"], Value::from(empty));
}

#[test]
fn validate_flags_broken_schemas() {
    let mut tool = serde_json::json!({
        "name": "x",
        "inputSchema": {
            "type": "string",
            "properties": {},
            "required": ["missing"]
        }
    });
    let defects = validate_tool_schema(&tool);
    assert_eq!(defects.len(), 2);
    tool.as_object_mut().expect("object").remove("inputSchema");
    assert_eq!(validate_tool_schema(&tool).len(), 1);
}

#[test]
fn pack_revision_is_pinned() {
    assert_eq!(pack()["mcp_protocol_revision"], MCP_PROTOCOL_REVISION);
    assert_eq!(MCP_PROTOCOL_REVISION, "2025-11-25");
}

#[test]
fn pack_publishes_no_roadmap_phase_number() {
    let text = pack().to_string();
    assert!(!text.contains("honored_from_phase"), "{text}");
    assert!(
        !text.to_ascii_lowercase().contains("phase 0"),
        "the published pack must not name a roadmap phase: {text}"
    );
}

/// A dangling `$ref` inside inputSchema is reported as a structural defect, so
/// an unresolvable JSON Pointer can never ship in the pack.
#[test]
fn validate_flags_unresolvable_ref_pointers() {
    let mut tool = serde_json::json!({
        "name": "x",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"$ref": "#/definitions/missing"}},
            "required": []
        }
    });
    let defects = validate_tool_schema(&tool);
    assert_eq!(defects.len(), 1, "{defects:?}");
    assert!(defects[0].contains("unresolvable $ref"), "{defects:?}");
    tool["inputSchema"]["definitions"] = serde_json::json!({"missing": {"type": "string"}});
    let after_definition = validate_tool_schema(&tool);
    assert!(after_definition.is_empty(), "{after_definition:?}");
    // A bare `#` self-reference resolves to the document root.
    tool["inputSchema"]["properties"]["a"] = serde_json::json!({"$ref": "#"});
    let after_self_ref = validate_tool_schema(&tool);
    assert!(after_self_ref.is_empty(), "{after_self_ref:?}");
}

/// One tool definition from the published schema pack.
fn tool_def(name: &str) -> Value {
    pack()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("{name} missing"))
        .clone()
}

#[test]
fn every_gated_write_tool_can_waive_the_duplicate_rejection() {
    // The exact-duplicate rejection applies to every gated long-term write, so
    // every one of these tools must be able to waive it. A tool whose schema
    // declares `additionalProperties: false` without this property rejects the
    // call at the client, which makes the escape hatch unusable on the MCP
    // surface even though the CLI accepts it.
    for tool in [
        "store",
        "admit_preview",
        "triple_add",
        "task_upsert",
        "failure_record",
        "persona_put_stable",
        "canonical_put",
    ] {
        let def = tool_def(tool);
        assert!(
            def["inputSchema"]["properties"]
                .get("allow_duplicate")
                .is_some(),
            "{tool} cannot waive the exact-duplicate rejection: {}",
            def["inputSchema"]
        );
    }
}

#[test]
fn context_property_says_it_is_not_searchable() {
    // An agent that tags an item `context: "goals"` and later recalls "my
    // goals" gets nothing, because context is never a retrieval signal. Every
    // tool carrying the property shares one description, so pinning it here
    // covers all of them.
    for tool in [
        "store",
        "admit_preview",
        "triple_add",
        "task_upsert",
        "failure_record",
        "persona_put_stable",
        "canonical_put",
    ] {
        let context = tool_def(tool)["inputSchema"]["properties"]["context"].clone();
        let desc = context["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} has no context description"));
        assert!(
            desc.contains("not searchable"),
            "{tool} must tell the model context is not searchable: {desc}"
        );
    }
}

#[test]
fn the_duplicate_waiver_is_named_for_what_it_waives() {
    // `consolidate` already owns an unrelated `force`. The duplicate waiver must
    // not reuse the name, or one flag means two things across the catalog and a
    // client can pass the wrong intent silently.
    assert!(
        tool_def("consolidate")["inputSchema"]["properties"]
            .get("force")
            .is_some(),
        "consolidate keeps its own force"
    );
    assert!(
        tool_def("store")["inputSchema"]["properties"]
            .get("force")
            .is_none(),
        "store must not borrow consolidate's force name"
    );
}

#[test]
fn consolidate_publishes_bank_as_its_selector_and_no_scope() {
    // The selector was renamed with no alias, so the published schema must not
    // offer `scope`: a conforming client rejects the old name instead of the
    // selector silently falling back to the context bank. `force` describes the
    // full wave it now performs, not only the dirty-path refresh.
    let properties = tool_def("consolidate")["inputSchema"]["properties"].clone();
    assert!(properties.get("scope").is_none(), "{properties}");
    let bank = properties["bank"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(bank.contains("Target bank"), "{bank}");
    let force = properties["force"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(force.contains("refresh"), "{force}");
    assert!(force.contains("reconcile"), "{force}");
}

#[test]
fn the_stats_row_names_its_lists_band_unit_and_tier_split() {
    // A caller reading only the published row must learn what the two lists
    // are, what unit the age bands use, that the band boundaries are visible
    // in the report, which pressure counts can appear, and that `tier` on this
    // surface means only the semantic-versus-episodic split.
    let row = tool_def("stats");
    let description = row["description"].as_str().expect("stats description");
    assert!(description.contains("`by_age_band`"), "{description}");
    assert!(
        description.contains("`by_provenance_class`"),
        "{description}"
    );
    assert!(description.contains("half-life"), "{description}");
    assert!(
        description.contains("boundaries in days appear in its label"),
        "{description}"
    );
    for count in ["flagged", "stale summaries", "archive candidates"] {
        assert!(description.contains(count), "{description}");
    }
    assert!(
        description.contains("semantic-versus-episodic"),
        "{description}"
    );
    assert!(description.contains("nothing else"), "{description}");
}
