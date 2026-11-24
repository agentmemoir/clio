// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the additive harness workspace
//! tool definitions: the batch operation element, the scratchpad family, the
//! declared required sets, and the published property enums.
//!
//! ## Owns
//! - Batch operation element shape and its recognized tool enum
//! - Scratchpad content property presence by tool
//! - Required argument sets and property enums of every additive tool
//!
//! ## Does not own
//! - Core or record-write definitions (see `schema_defs`, `schema_record_defs`)
//! - Pack assembly, hashing, or export (see `schema`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the additive tool schema definitions.

use super::*;

/// One additive tool definition by name.
fn additive_def(name: &str) -> ToolDef {
    additive_defs()
        .into_iter()
        .find(|def| def.name == name)
        .unwrap_or_else(|| panic!("{name} missing from the additive schema pack"))
}

/// The property names of one definition, in declaration order.
fn prop_names(def: &ToolDef) -> Vec<&'static str> {
    def.props.iter().map(|(name, _)| *name).collect()
}

/// The enum values of one property of one tool definition.
fn enum_values(def: &ToolDef, prop_name: &str) -> Vec<String> {
    def.props
        .iter()
        .find(|(name, _)| *name == prop_name)
        .unwrap_or_else(|| panic!("`{}` has no `{prop_name}` property", def.name))
        .1["enum"]
        .as_array()
        .expect("enum array")
        .iter()
        .map(|value| value.as_str().expect("string enum value").to_owned())
        .collect()
}

#[test]
fn batch_operation_names_the_recognized_mutating_tools() {
    let op = batch_op();
    let names: Vec<&str> = op["properties"]["tool"]["enum"]
        .as_array()
        .expect("tool enum")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        names,
        [
            "store",
            "triple_add",
            "canonical_put",
            "persona_put_stable",
            "task_upsert",
            "failure_record",
            "graph_link",
        ]
    );
}

#[test]
fn batch_operation_requires_only_the_wrapped_tool() {
    assert_eq!(batch_op()["required"], serde_json::json!(["tool"]));
}

#[test]
fn batch_operation_carries_the_wrapped_parameters_as_an_open_object() {
    let params = &batch_op()["properties"]["params"];
    assert_eq!(params["type"], serde_json::json!("object"));
    assert!(
        params.get("additionalProperties").is_none(),
        "the wrapped tool's own schema decides its arguments: {params}"
    );
}

#[test]
fn batch_operation_refuses_properties_it_does_not_declare() {
    assert_eq!(batch_op()["additionalProperties"], serde_json::json!(false));
}

#[test]
fn batch_tool_requires_only_the_operations_list() {
    assert_eq!(additive_def("batch").required, ["operations"].as_slice());
}

#[test]
fn batch_tool_no_longer_declares_the_duplicate_waiver() {
    let tool = additive_def("batch").to_tool();
    let properties = tool["inputSchema"]["properties"]
        .as_object()
        .expect("batch properties");
    assert!(
        !properties.contains_key("allow_duplicate"),
        "batch still publishes the retired duplicate waiver: {properties:?}"
    );
}

#[test]
fn batch_tool_publishes_the_operation_element_under_operations() {
    let tool = additive_def("batch").to_tool();
    assert_eq!(
        tool["inputSchema"]["properties"]["operations"]["items"],
        batch_op()
    );
}

#[test]
fn scratchpad_write_adds_the_content_property() {
    let def = scratchpad_like("scratchpad_write", "notes", &["key", "content"]);
    assert!(prop_names(&def).contains(&"content"));
    assert_eq!(def.required, ["key", "content"].as_slice());
}

#[test]
fn scratchpad_read_omits_the_content_property() {
    let def = scratchpad_like("scratchpad_read", "notes", &[]);
    assert!(!prop_names(&def).contains(&"content"));
    assert_eq!(def.required.len(), 0);
}

#[test]
fn scratchpad_tools_carry_the_key_session_and_attribution_properties() {
    let def = scratchpad_like("scratchpad_clear", "notes", &[]);
    assert_eq!(prop_names(&def), ["key", "session", "bank", "actor"]);
}

#[test]
fn additive_defs_publishes_each_additive_tool_once() {
    let mut names: Vec<&str> = additive_defs().iter().map(|def| def.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "batch",
            "canonical_get",
            "canonical_put",
            "scratchpad_clear",
            "scratchpad_read",
            "scratchpad_write",
            "shared_discard",
            "shared_retrieve",
            "shared_store",
            "stats",
            "validate",
        ]
    );
}

#[test]
fn additive_defs_requires_the_declared_arguments_per_tool() {
    let cases: [(&str, &[&str]); 8] = [
        ("batch", &["operations"]),
        (
            "canonical_put",
            &["key", "value", "category", "epistemic_kind"],
        ),
        ("canonical_get", &["key"]),
        ("shared_store", &["item", "epistemic_kind", "actor"]),
        ("shared_retrieve", &["query", "actor"]),
        ("shared_discard", &["item_id", "reason", "actor"]),
        ("validate", &["item_id", "action"]),
        ("stats", &[]),
    ];
    for (name, required) in cases {
        assert_eq!(additive_def(name).required, required, "{name}");
    }
}

#[test]
fn validate_offers_delete_as_a_documented_discard_alias() {
    let def = additive_def("validate");
    assert_eq!(
        enum_values(&def, "action"),
        ["attest", "update", "invalidate", "discard", "delete"]
    );
    let action = def
        .props
        .iter()
        .find(|(name, _)| *name == "action")
        .expect("action property")
        .1
        .clone();
    assert!(
        action["description"]
            .as_str()
            .unwrap_or_default()
            .contains("alias"),
        "{action}"
    );
}

#[test]
fn canonical_put_limits_the_snapshot_to_its_documented_leaves() {
    let snapshot = additive_def("canonical_put")
        .props
        .iter()
        .find(|(name, _)| *name == "snapshot")
        .expect("snapshot property")
        .1
        .clone();
    assert_eq!(snapshot["type"], serde_json::json!("object"));
    let description = snapshot["description"].as_str().unwrap_or_default();
    for leaf in ["entity", "amount", "date"] {
        assert!(description.contains(leaf), "snapshot omits `{leaf}`");
    }
}
