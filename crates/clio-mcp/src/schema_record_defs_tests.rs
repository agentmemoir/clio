// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the persona, history, triple,
//! and belief record-write tool definitions: the published tool set, the
//! declared required arguments, the shared bounded context property, and the
//! property enums the declarations produce.
//!
//! ## Owns
//! - The record-write tool name set and required argument sets
//! - The absence of the retired duplicate waiver in every record declaration
//! - The `update_rule`, persona `source`, and task `steps` property shapes
//!
//! ## Does not own
//! - Item-level or read tool definitions (see `schema_defs`, `schema_read_defs`)
//! - Pack assembly, hashing, or export (see `schema`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the record-write tool declarations.

use super::*;
use serde_json::Value;

/// One record-write tool definition by name.
fn record_def(name: &str) -> ToolDef {
    record_defs()
        .into_iter()
        .find(|def| def.name == name)
        .unwrap_or_else(|| panic!("{name} missing from the record schema pack"))
}

/// The schema of one property of one tool definition.
fn prop(def: &ToolDef, prop_name: &str) -> Value {
    def.props
        .iter()
        .find(|(name, _)| *name == prop_name)
        .unwrap_or_else(|| panic!("`{}` has no `{prop_name}` property", def.name))
        .1
        .clone()
}

/// The enum values of one property of one tool definition.
fn enum_values(def: &ToolDef, prop_name: &str) -> Vec<String> {
    prop(def, prop_name)["enum"]
        .as_array()
        .expect("enum array")
        .iter()
        .map(|value| value.as_str().expect("string enum value").to_owned())
        .collect()
}

#[test]
fn record_defs_publishes_each_record_write_tool_once() {
    let mut names: Vec<&str> = record_defs().iter().map(|def| def.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "belief_observe",
            "failure_record",
            "persona_observe_preference",
            "persona_put_stable",
            "task_upsert",
            "triple_add",
            "triple_end",
        ]
    );
}

#[test]
fn record_defs_requires_the_declared_arguments_per_tool() {
    let cases: [(&str, &[&str]); 7] = [
        ("persona_put_stable", &["key", "value"]),
        ("persona_observe_preference", &["key", "observed_value"]),
        ("task_upsert", &["task_id", "definition", "status"]),
        (
            "failure_record",
            &["task_id", "attempt_n", "what_failed", "lesson"],
        ),
        (
            "triple_add",
            &["subject", "predicate", "object", "epistemic_kind"],
        ),
        ("triple_end", &["subject", "predicate", "valid_until"]),
        (
            "belief_observe",
            &["proposition", "confidence", "evidence_ref", "source_type"],
        ),
    ];
    for (name, required) in cases {
        assert_eq!(record_def(name).required, required, "{name}");
    }
}

#[test]
fn no_record_tool_declares_the_duplicate_waiver() {
    for def in record_defs() {
        let tool = def.to_tool();
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} declares no properties", def.name));
        assert!(
            !properties.contains_key("allow_duplicate"),
            "{} still publishes the retired duplicate waiver",
            def.name
        );
    }
}

#[test]
fn triple_tools_limit_update_rule_to_the_declared_set() {
    for name in ["triple_add", "triple_end"] {
        assert_eq!(
            enum_values(&record_def(name), "update_rule"),
            ["discrete", "continuous"],
            "{name}"
        );
    }
}

#[test]
fn persona_tools_offer_the_same_source_enum() {
    for name in ["persona_put_stable", "persona_observe_preference"] {
        assert_eq!(
            enum_values(&record_def(name), "source"),
            ["explicit", "inferred"],
            "{name}"
        );
    }
}

#[test]
fn task_upsert_stages_ordered_string_steps() {
    let steps = prop(&record_def("task_upsert"), "steps");
    assert_eq!(steps["type"], serde_json::json!("array"));
    assert_eq!(steps["items"]["type"], serde_json::json!("string"));
}

#[test]
fn record_tools_share_the_bounded_context_property() {
    for name in [
        "persona_put_stable",
        "persona_observe_preference",
        "task_upsert",
        "failure_record",
        "triple_add",
        "belief_observe",
    ] {
        let context = prop(&record_def(name), "context");
        assert_eq!(context["type"], serde_json::json!("string"), "{name}");
        assert_eq!(context["maxLength"], serde_json::json!(4096), "{name}");
    }
}

#[test]
fn triple_add_requires_an_epistemic_kind_from_the_declared_set() {
    assert_eq!(
        enum_values(&record_def("triple_add"), "epistemic_kind"),
        ["fact", "belief"]
    );
}
