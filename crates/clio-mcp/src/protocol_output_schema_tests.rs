// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that each declared `outputSchema`
//! validates the value its handler really returns.
//!
//! ## Owns
//! - A small JSON Schema subset validator that reports the failing path
//! - The keyword guard over every declared schema
//! - The dispatch-and-validate sweep over every published tool
//!
//! ## Does not own
//! - Schema definitions or the registry data file (see `schema_output_defs`)
//! - Tool dispatch semantics (see `write_tools` and the tool modules)
//!
//! ## Boundary
//!
//! The validator supports only the keywords declared in [`ALLOWED_KEYWORDS`].
//! Adding a keyword to a schema must fail the guard test first, so no schema
//! can escape validation through an unsupported keyword.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use crate::schema::schema_pack;
use serde_json::{Value, json};
use std::sync::Arc;

/// Keywords the validator understands. The guard test rejects any other key.
const ALLOWED_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "anyOf",
    "const",
    "enum",
    "items",
    "oneOf",
    "properties",
    "required",
    "type",
];

/// Does the JSON value satisfy one JSON Schema type name?
fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Validate `value` against the supported schema subset.
///
/// # Errors
/// Returns the JSON path of the first mismatch, with the expected node and the
/// actual value.
fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    validate_at(schema, value, "$")
}

fn validate_at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    // An empty schema (`{}`) accepts every value.
    let Value::Object(fields) = schema else {
        return Ok(());
    };
    if let Some(kind) = fields.get("type") {
        let ok = match kind {
            Value::String(name) => type_matches(name, value),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .any(|name| type_matches(name, value)),
            _ => true,
        };
        if !ok {
            return Err(format!("{path}: type {kind} does not match {value}"));
        }
    }
    if let Some(expected) = fields.get("const")
        && expected != value
    {
        return Err(format!("{path}: expected const {expected}, got {value}"));
    }
    if let Some(allowed) = fields.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(format!("{path}: {value} is not one of {allowed:?}"));
    }
    if let Some(object) = value.as_object() {
        for name in fields
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(name) {
                return Err(format!("{path}: missing required property `{name}`"));
            }
        }
        let props = fields.get("properties").and_then(Value::as_object);
        for (name, child_schema) in props.into_iter().flatten() {
            if let Some(child) = object.get(name) {
                validate_at(child_schema, child, &format!("{path}.{name}"))?;
            }
        }
        let is_extra = |key: &String| props.is_none_or(|props| !props.contains_key(key));
        match fields.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                if let Some(key) = object.keys().find(|key| is_extra(key)) {
                    return Err(format!("{path}: unexpected property `{key}`"));
                }
            }
            Some(extra_schema @ Value::Object(_)) => {
                for (key, child) in object {
                    if is_extra(key) {
                        validate_at(extra_schema, child, &format!("{path}.{key}"))?;
                    }
                }
            }
            _ => {}
        }
    }
    if let (Some(items), Some(array)) = (fields.get("items"), value.as_array()) {
        for (index, child) in array.iter().enumerate() {
            validate_at(items, child, &format!("{path}[{index}]"))?;
        }
    }
    if let Some(branches) = fields.get("oneOf").and_then(Value::as_array) {
        let matched = branches
            .iter()
            .filter(|branch| validate_at(branch, value, path).is_ok())
            .count();
        if matched != 1 {
            return Err(format!(
                "{path}: oneOf matched {matched} branches, expected 1"
            ));
        }
    }
    if let Some(branches) = fields.get("anyOf").and_then(Value::as_array)
        && !branches
            .iter()
            .any(|branch| validate_at(branch, value, path).is_ok())
    {
        return Err(format!(
            "{path}: anyOf matched no branch out of {}",
            branches.len()
        ));
    }
    Ok(())
}

/// Walk one schema tree and panic on a key the validator does not support.
fn assert_keywords_supported(tool: &str, schema: &Value) {
    let Value::Object(fields) = schema else {
        panic!("{tool}: a schema node must be an object, got {schema}");
    };
    for (key, child) in fields {
        assert!(
            ALLOWED_KEYWORDS.contains(&key.as_str()),
            "`{tool}` uses unsupported schema keyword `{key}`; add it to the validator before declaring it"
        );
        match key.as_str() {
            "properties" => {
                for node in child.as_object().expect("properties object").values() {
                    assert_keywords_supported(tool, node);
                }
            }
            "items" => assert_keywords_supported(tool, child),
            "oneOf" => {
                for node in child.as_array().expect("oneOf array") {
                    assert_keywords_supported(tool, node);
                }
            }
            "additionalProperties" if child.is_object() => {
                assert_keywords_supported(tool, child);
            }
            _ => {}
        }
    }
}

#[test]
fn protocol_output_schema_validator_reports_the_failing_path() {
    let schema = json!({
        "type": "object",
        "properties": {
            "ok": {"type": "boolean"},
            "count": {"type": "integer"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "mode": {"enum": ["a", "b"]},
            "either": {"oneOf": [{"type": "string"}, {"type": "null"}]},
            "overlap": {"anyOf": [
                {"type": "object", "required": ["confirm"]},
                {"type": "object", "required": ["would_correct"]}
            ]},
            "closed": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        "required": ["ok"]
    });
    assert!(validate(&schema, &json!({"ok": true, "count": 2, "tags": ["x"], "mode": "a", "either": null, "closed": {}})).is_ok());
    let mismatch = validate(&schema, &json!({"ok": true, "tags": ["x", 3]}))
        .expect_err("numeric tag must be rejected");
    assert_eq!(mismatch, "$.tags[1]: type \"string\" does not match 3");
    let missing = validate(&schema, &json!({})).expect_err("`ok` is required");
    assert_eq!(missing, "$: missing required property `ok`");
    let unexpected = validate(&schema, &json!({"ok": true, "closed": {"x": 1}}))
        .expect_err("closed object must reject an extra key");
    assert_eq!(unexpected, "$.closed: unexpected property `x`");
    let enum_miss = validate(&schema, &json!({"ok": true, "mode": "c"}))
        .expect_err("enum mismatch must be rejected");
    assert!(enum_miss.starts_with("$.mode: "), "{enum_miss}");
    let one_of_miss = validate(&schema, &json!({"ok": true, "either": 5}))
        .expect_err("oneOf mismatch must be rejected");
    assert!(
        one_of_miss.starts_with("$.either: oneOf matched 0"),
        "{one_of_miss}"
    );
    // anyOf accepts a value that matches more than one branch, which is the
    // case `oneOf` would reject.
    assert!(
        validate(
            &schema,
            &json!({"ok": true, "overlap": {"confirm": false, "would_correct": {}}})
        )
        .is_ok()
    );
    let any_of_miss = validate(&schema, &json!({"ok": true, "overlap": {"other": 1}}))
        .expect_err("anyOf mismatch must be rejected");
    assert_eq!(any_of_miss, "$.overlap: anyOf matched no branch out of 2");
}

/// Guard: every declared schema uses only keywords the validator understands.
///
/// A new keyword in any schema fails this test, so no schema can escape
/// validation by using a keyword that is silently ignored.
#[test]
fn protocol_output_schema_guard_rejects_unvalidated_keywords() {
    let pack = schema_pack();
    let tools = pack["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "the pack must publish tools");
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("{name}: no declared outputSchema"));
        assert_keywords_supported(name, schema);
    }
}

/// Dispatch every published tool with real minimal arguments and validate the
/// value the handler returns against that tool's declared `outputSchema`.
///
/// A failure envelope is a real value too: the schema must accept it.
#[test]
fn protocol_output_schema_accepts_the_handled_value_for_every_tool() {
    let state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    let handler = McpHandler::new(Arc::clone(&state));
    let pack = schema_pack();
    let tools = pack["tools"].as_array().expect("tools array");
    let mut validated = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("{name}: no declared outputSchema"));
        let args = super::protocol_nfr7_tests::minimal_args(tool);
        let response = handler
            .handle_message(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }))
            .unwrap_or_else(|| panic!("{name}: tools/call produced no response"));
        if let Some(error) = response.get("error") {
            panic!("{name}: tools/call failed at the RPC layer: {error}");
        }
        let value = &response["result"]["structuredContent"];
        assert!(
            !value.is_null(),
            "{name}: tools/call returned no structuredContent: {response}"
        );
        match validate(schema, value) {
            Ok(()) => validated += 1,
            Err(mismatch) => failures.push(format!("{name}: {mismatch}\n  value: {value}")),
        }
    }
    assert!(
        failures.is_empty(),
        "declared outputSchema rejects the handled value for {} of {} tools:\n{}",
        failures.len(),
        tools.len(),
        failures.join("\n")
    );
    assert_eq!(
        validated,
        tools.len(),
        "every published tool must be validated"
    );
    assert!(validated > 0, "the pack must publish tools");
}
