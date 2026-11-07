// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin `tool_describe` to the
//! full catalog: the exact definition it returns, the profile it ignores, the
//! shared refusal for an unknown name, and the arguments it accepts.
//!
//! ## Owns
//! - The returned definition equalling the catalog definition
//! - A tool the selected profile hides answering like the all-profile call
//! - The shared unknown-tool refusal, including self-describe
//! - The response keys and the published argument rules
//! - Every described schema building in the offline validator
//! - The call envelope carrying the definition as structured content
//!
//! ## Does not own
//! - What a profile advertises (`tool_profile_tests`)
//! - What a tool's schema declares (`schema_defs` tests)
//!
//! ## Boundary
//!
//! Changes to what `tool_describe` returns should normally require changes
//! here. Changes to what a described tool declares should not.
//!
//! Keep this module focused on describing one catalog tool.

use crate::protocol::{ERR_INVALID_PARAMS, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use crate::schema::{catalog_def, catalog_defs, schema_pack};
use serde_json::{Value, json};
use std::sync::Arc;

/// A server state whose configuration layer is exactly `config`.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// The advertised tool names for one state.
fn listed(state: &McpState) -> Vec<String> {
    let value = McpHandler::tools_list_visible(state).expect("a known profile lists its tools");
    value["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The whole JSON-RPC response of one `tools/call`.
fn call(state: &Arc<McpState>, name: &str, args: &Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": args }
    });
    McpHandler::new(Arc::clone(state))
        .handle_message(&request)
        .expect("a request with an id has a response")
}

/// The structured content of a successful `tool_describe` call.
fn described(state: &Arc<McpState>, name: &str) -> Value {
    call(state, "tool_describe", &json!({ "name": name }))["result"]["structuredContent"].clone()
}

/// The `tool_describe` declaration this build publishes.
fn declaration() -> &'static Value {
    crate::discovery_tool_defs::definitions()["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .find(|tool| tool["name"] == json!("tool_describe"))
        .expect("the document declares tool_describe")
}

#[test]
fn the_description_is_the_catalog_definition() {
    let state = state_with("{}");
    let pack = schema_pack();
    let tools = pack["tools"].as_array().expect("tools");
    for name in ["store", "admit_preview_batch", "retrieve"] {
        let expected = catalog_def(name)
            .expect("the full catalog defines the tool")
            .to_tool();
        assert_eq!(
            described(&state, name),
            expected,
            "`{name}` must render its exact catalog definition"
        );
        let from_pack = tools
            .iter()
            .find(|tool| tool["name"] == json!(name))
            .expect("the schema pack publishes the tool");
        assert_eq!(described(&state, name), *from_pack, "`{name}` pack drift");
    }
    assert!(
        catalog_def("store").expect("store").to_tool()["outputSchema"].is_object(),
        "store declares an outputSchema"
    );
    assert!(
        catalog_def("admit_preview_batch").expect("batch").to_tool()["inputSchema"]["$defs"]
            .is_object(),
        "admit_preview_batch carries $defs"
    );
}

#[test]
fn a_tool_the_profile_hides_is_answered_the_same_way() {
    let hidden = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let all = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    assert!(
        !listed(&hidden).contains(&"store".to_owned()),
        "the minimal profile hides store"
    );
    assert!(
        listed(&all).contains(&"store".to_owned()),
        "the all profile lists store"
    );
    assert_eq!(
        described(&hidden, "store"),
        described(&all, "store"),
        "the profile must not change the definition"
    );
    assert_eq!(
        described(&hidden, "store"),
        catalog_def("store").expect("store").to_tool()
    );
}

#[test]
fn an_unknown_name_is_the_shared_unknown_tool_refusal() {
    let state = state_with("{}");
    let unknown = call(&state, "tool_describe", &json!({ "name": "no_such_tool" }));
    let gate = call(&state, "no_such_tool", &json!({}));
    assert_eq!(
        unknown["error"], gate["error"],
        "an unknown name must use the gate's refusal"
    );
    for name in ["no_such_tool", "tool_describe"] {
        let response = call(&state, "tool_describe", &json!({ "name": name }));
        let error = &response["error"];
        assert_eq!(error["code"], json!(ERR_INVALID_PARAMS), "{name}");
        assert_eq!(error["message"], json!(format!("unknown tool `{name}`")));
    }
}

#[test]
fn the_response_holds_only_the_described_tool() {
    let state = state_with("{}");
    let payload = described(&state, "store");
    let fields = payload.as_object().expect("a definition object");
    for key in fields.keys() {
        assert!(
            ["name", "description", "inputSchema", "outputSchema"].contains(&key.as_str()),
            "unexpected response key `{key}`"
        );
    }
    assert!(fields.get("tools").is_none(), "no nested tool list");
    assert_eq!(fields.get("name"), Some(&json!("store")));
}

#[test]
fn the_arguments_are_the_published_name_field_only() {
    let state = state_with("{}");
    for args in [
        json!({}),
        json!({ "name": "" }),
        json!({ "name": 5 }),
        json!({ "name": "store", "extra": 1 }),
        json!(null),
        json!([]),
        json!("x"),
    ] {
        let response = call(&state, "tool_describe", &args);
        assert_eq!(
            response["error"]["code"],
            json!(ERR_INVALID_PARAMS),
            "{args} must be refused"
        );
        assert!(
            matches!(response["error"]["message"].as_str(), Some(message) if !message.is_empty()),
            "{args} must carry a message"
        );
        assert!(response.get("result").is_none(), "{args}");
    }
    // A request with no `arguments` key at all is refused the same way.
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "tool_describe" }
    });
    let response = McpHandler::new(Arc::clone(&state))
        .handle_message(&request)
        .expect("a request with an id has a response");
    assert_eq!(
        response["error"]["code"],
        json!(ERR_INVALID_PARAMS),
        "a missing arguments key must be refused"
    );
    assert!(
        matches!(response["error"]["message"].as_str(), Some(message) if !message.is_empty()),
        "a missing arguments key must carry a message"
    );
    assert!(response.get("result").is_none());
    let declared = &declaration()["inputSchema"];
    assert_eq!(declared["required"], json!(["name"]));
    assert_eq!(declared["properties"]["name"]["minLength"], json!(1));
    assert_eq!(declared["additionalProperties"], json!(false));
}

#[test]
fn every_described_schema_resolves_with_no_network() {
    let defs = catalog_defs();
    assert!(
        !defs.is_empty(),
        "the catalogue must define at least one tool"
    );
    for def in defs {
        let tool = def.to_tool();
        let input = &tool["inputSchema"];
        assert!(
            jsonschema::draft202012::meta::is_valid(input),
            "{}: inputSchema is not draft 2020-12",
            def.name
        );
        jsonschema::options()
            .offline()
            .build(input)
            .unwrap_or_else(|error| panic!("{}: inputSchema does not build: {error}", def.name));
        // This loop covers each output schema when the tool declares one.
        if let Some(output) = tool.get("outputSchema") {
            assert!(
                jsonschema::draft202012::meta::is_valid(output),
                "{}: outputSchema is not draft 2020-12",
                def.name
            );
            jsonschema::options()
                .offline()
                .build(output)
                .unwrap_or_else(|error| {
                    panic!("{}: outputSchema does not build: {error}", def.name)
                });
            assert!(
                clio_config::retention_schema::schema_refs_resolve(output),
                "{}: outputSchema has an unresolved reference",
                def.name
            );
        }
        assert!(
            clio_config::retention_schema::schema_refs_resolve(input),
            "{}: inputSchema has an unresolved reference",
            def.name
        );
    }
}

#[test]
fn the_discovery_names_are_not_bound_tools() {
    let bound = clio_types::mcp_tools::bound_tools();
    for name in [
        "tool_search",
        crate::discovery_tools::TOOL_DESCRIBE,
        "tool_invoke",
    ] {
        assert!(
            !bound.contains(&name),
            "`{name}` must stay outside the bound set"
        );
    }
}

#[test]
fn the_call_envelope_carries_the_definition_as_structured_content() {
    let state = state_with("{}");
    let response = call(&state, "tool_describe", &json!({ "name": "store" }));
    let result = &response["result"];
    assert_eq!(result["isError"], json!(false));
    assert_eq!(result["content"][0]["type"], json!("text"));
    assert_eq!(result["content"][0]["text"], json!("ok"));
    assert_eq!(
        result["structuredContent"],
        catalog_def("store").expect("store").to_tool()
    );
}
