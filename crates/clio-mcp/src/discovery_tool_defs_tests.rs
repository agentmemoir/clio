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
//! - The refused recursive targets on `tool_invoke` and the declared tool set
//! - The code every invalid example declares, in the shape its tool answers
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
    assert_eq!(document["json_schema_dialect"], "2020-12");
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

/// The codes a `tools/call` result payload carries for a bad argument. A call
/// with bad arguments answers an `isError` result, matching the direct call it
/// is held to, so these are the only codes the wrapper can answer.
const TOOL_RESULT_CODES: [&str; 2] = ["invalid_argument", "invalid_type"];

/// Each invalid example declares a code: the payload code a tool whose handler
/// shares the direct call's parity answers, and the JSON-RPC code for a tool
/// this build does not route yet.
#[test]
fn every_invalid_example_declares_a_code_its_tool_answers() {
    for tool in tools() {
        let name = tool["name"].as_str().expect("a tool name");
        for example in tool["examples"]["invalid"]
            .as_array()
            .expect("invalid examples")
        {
            let declared = &example["code"];
            if name == crate::catalog_invoke::TOOL_INVOKE {
                let code = declared
                    .as_str()
                    .unwrap_or_else(|| panic!("{name}: {declared} is not a tool-result code"));
                assert!(
                    TOOL_RESULT_CODES.contains(&code),
                    "{name}: `{code}` is not a code a tools/call result carries"
                );
            } else {
                assert_eq!(
                    declared,
                    crate::protocol::ERR_INVALID_PARAMS,
                    "{name}: an invalid example must declare the invalid-params code"
                );
            }
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
fn invoke_refuses_search_describe_and_invoke_targets() {
    for name in TOOL_NAMES {
        let refused = tool(name)["examples"].get("refused");
        assert_eq!(
            refused.is_some(),
            name == "tool_invoke",
            "{name}: only tool_invoke declares refused targets"
        );
    }
    let expected: Vec<Value> = TOOL_NAMES
        .iter()
        .map(|name| Value::String((*name).to_owned()))
        .collect();
    let refused = tool("tool_invoke")["examples"]["refused"]
        .as_array()
        .expect("tool_invoke declares refused examples");
    let targets: Vec<Value> = refused
        .iter()
        .map(|entry| entry["arguments"]["name"].clone())
        .collect();
    assert_eq!(targets, expected, "refused targets changed");
}

#[test]
fn every_refused_example_is_valid_input_for_its_owning_tool() {
    for tool in tools() {
        let Some(refused) = tool["examples"].get("refused").and_then(Value::as_array) else {
            continue;
        };
        let validator = validator(&tool["inputSchema"]);
        for entry in refused {
            assert!(
                validator.is_valid(&entry["arguments"]),
                "{}: refused example {} is not valid input",
                tool["name"],
                entry["label"]
            );
        }
    }
}

#[test]
fn no_schema_declares_a_schema_keyword() {
    for tool in tools() {
        for key in ["inputSchema", "outputSchema"] {
            assert!(
                tool[key].get("$schema").is_none(),
                "{}: {key} declares $schema",
                tool["name"]
            );
        }
    }
}

#[test]
fn describe_declares_the_existing_unknown_name_refusal() {
    let refusal = &tool("tool_describe")["refusals"]["unknownName"];
    assert_eq!(
        refusal["code"].as_i64(),
        Some(crate::protocol::ERR_INVALID_PARAMS)
    );
    assert_eq!(refusal["messageTemplate"], "unknown tool `{name}`");
}

#[test]
fn all_three_tools_are_declared() {
    let declared: Vec<&str> = tools()
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    assert_eq!(declared, TOOL_NAMES);
}

/// Every published listing entry carries exactly the four contract keys, so no
/// declaration-only field leaks into `tools/list`.
#[test]
fn a_listing_entry_carries_only_the_four_published_keys() {
    for tool in tools() {
        let name = tool["name"].as_str().expect("a tool name");
        let entry = crate::discovery_tool_defs::entry(name);
        let object = entry.as_object().expect("an entry object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["description", "inputSchema", "name", "outputSchema"],
            "`{name}` published {keys:?}"
        );
    }
}
