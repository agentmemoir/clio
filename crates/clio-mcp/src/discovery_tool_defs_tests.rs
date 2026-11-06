// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the embedded discovery
//! tool declarations to the dialect, the tool-group table, and the argument
//! rules they publish.
//!
//! ## Owns
//! - The declared format, revision, dialect, and catalog scope
//! - Draft 2020-12 validity of every declared input and output schema
//! - Every valid example passing and every invalid example failing with the
//!   keyword and instance path it declares
//! - The family and group enums matching the tool-group table
//! - The declared argument rules producing the stated effective arguments
//! - The refused recursive targets and the declared tool set
//!
//! ## Does not own
//! - Catalog registration, dispatch, or ranking for these tools
//! - The business tool schemas themselves (`schema_defs`)
//!
//! ## Boundary
//!
//! Changes to a discovery tool's declared contract should normally require
//! changes to the embedded data file and then to these tests. Changes to the
//! tool-group table should require no change here.
//!
//! Keep this module focused on the discovery tool declarations.

use serde_json::Value;

/// The three discovery tools this document declares.
const TOOL_NAMES: [&str; 3] = ["tool_search", "tool_describe", "tool_invoke"];

fn document() -> &'static Value {
    crate::discovery_tool_defs::definitions()
}

fn tools() -> &'static Vec<Value> {
    document()["tools"]
        .as_array()
        .expect("the document declares a tools array")
}

fn tool(name: &str) -> &'static Value {
    tools()
        .iter()
        .find(|tool| tool["name"] == Value::String(name.to_owned()))
        .unwrap_or_else(|| panic!("the document declares {name}"))
}

/// The group identifiers the tool-group table publishes, in table order.
fn group_names() -> Vec<Value> {
    clio_types::ToolGroup::ALL
        .iter()
        .map(|group| Value::String(group.as_str().to_owned()))
        .collect()
}

fn validator(schema: &Value) -> jsonschema::Validator {
    jsonschema::options()
        .offline()
        .build(schema)
        .unwrap_or_else(|error| panic!("schema does not compile offline: {error}"))
}

#[test]
fn format_and_dialect_are_declared() {
    let document = document();
    assert_eq!(document["format"], "am_mcp_discovery_tool_defs");
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["mcp_protocol_revision"], "2025-11-25");
    assert_eq!(
        document["json_schema_dialect"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(document["catalogScope"], "full");
}

#[test]
fn every_input_schema_is_draft_2020_12() {
    for tool in tools() {
        let schema = &tool["inputSchema"];
        assert!(
            jsonschema::draft202012::meta::is_valid(schema),
            "{}: inputSchema is not a valid draft 2020-12 schema",
            tool["name"]
        );
        validator(schema);
    }
}

#[test]
fn every_output_schema_is_draft_2020_12() {
    for tool in tools() {
        let schema = &tool["outputSchema"];
        assert!(
            jsonschema::draft202012::meta::is_valid(schema),
            "{}: outputSchema is not a valid draft 2020-12 schema",
            tool["name"]
        );
        validator(schema);
    }
}

#[test]
fn every_valid_example_validates() {
    for tool in tools() {
        let name = &tool["name"];
        let validator = validator(&tool["inputSchema"]);
        let examples = tool["examples"]["valid"]
            .as_array()
            .expect("valid examples");
        assert!(!examples.is_empty(), "{name}: no valid example is declared");
        for example in examples {
            assert!(
                validator.is_valid(example),
                "{name}: a valid example is rejected: {example}"
            );
        }
    }
}

#[test]
fn every_invalid_example_fails_with_the_declared_keyword() {
    for tool in tools() {
        let name = &tool["name"];
        let validator = validator(&tool["inputSchema"]);
        let examples = tool["examples"]["invalid"]
            .as_array()
            .expect("invalid examples");
        assert!(
            !examples.is_empty(),
            "{name}: no invalid example is declared"
        );
        for example in examples {
            let input = &example["input"];
            let errors: Vec<_> = validator.iter_errors(input).collect();
            let error = errors
                .first()
                .unwrap_or_else(|| panic!("{name}: invalid example passed: {input}"));
            assert_eq!(
                error.kind().keyword(),
                example["keyword"].as_str().expect("a declared keyword"),
                "{name}: wrong keyword for {input}"
            );
            assert_eq!(
                error.instance_path().as_str(),
                example["instancePath"]
                    .as_str()
                    .expect("a declared instance path"),
                "{name}: wrong instance path for {input}"
            );
        }
    }
}

#[test]
fn every_invalid_example_declares_the_invalid_params_code() {
    for tool in tools() {
        for example in tool["examples"]["invalid"]
            .as_array()
            .expect("invalid examples")
        {
            assert_eq!(
                example["code"], -32602,
                "{}: an invalid example must declare the invalid-params code",
                tool["name"]
            );
        }
    }
}

#[test]
fn family_enum_matches_the_tool_group_table() {
    let search = tool("tool_search");
    let expected = Value::Array(group_names());
    assert_eq!(
        search["inputSchema"]["properties"]["family"]["enum"],
        expected
    );
}

#[test]
fn output_group_enum_matches_the_tool_group_table() {
    let search = tool("tool_search");
    let group = &search["outputSchema"]["properties"]["tools"]["items"]["properties"]["group"];
    assert_eq!(group["enum"], Value::Array(group_names()));
}

/// The effective limit the declared rule produces for `input`.
fn effective_limit(rule: &Value, input: &Value) -> i64 {
    match input.get("limit").and_then(Value::as_i64) {
        Some(limit) => limit.min(
            rule["maximum"]
                .as_i64()
                .expect("the limit rule declares a maximum"),
        ),
        None => rule["default"]
            .as_i64()
            .expect("the limit rule declares a default"),
    }
}

#[test]
fn limit_rule_clamps_to_the_declared_maximum() {
    let search = tool("tool_search");
    let rule = &search["argumentRules"]["limit"];
    assert_eq!(rule["clamp"], true);
    let limits: Vec<i64> = search["examples"]["valid"]
        .as_array()
        .expect("valid examples")
        .iter()
        .map(|example| effective_limit(rule, example))
        .collect();
    assert_eq!(limits, vec![5, 10, 25, 10, 10]);
}

#[test]
fn arguments_rule_defaults_to_an_empty_object() {
    let rule = &tool("tool_invoke")["argumentRules"]["arguments"];
    assert_eq!(rule["default"], serde_json::json!({}));
}

#[test]
fn search_describe_and_invoke_targets_are_declared_as_refused() {
    let expected: Vec<Value> = TOOL_NAMES
        .iter()
        .map(|name| Value::String((*name).to_owned()))
        .collect();
    for name in TOOL_NAMES {
        let refused = tool(name)["examples"]["refused"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: no refused examples are declared"));
        let targets: Vec<Value> = refused
            .iter()
            .map(|entry| entry["arguments"]["name"].clone())
            .collect();
        assert_eq!(targets, expected, "{name}: refused targets changed");
    }
}

#[test]
fn all_three_tools_are_declared() {
    let declared: Vec<&str> = tools()
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    assert_eq!(declared, TOOL_NAMES);
}
