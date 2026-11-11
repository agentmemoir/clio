// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin exact catalog lookup and
//! the argument contract to the full catalog, so no tool profile and no visible
//! listing can hide a definition or change the contract registered for it.
//!
//! ## Owns
//! - An exact lookup resolving a tool the selected profile hides
//! - The contract set equalling the bound catalog plus the published discovery
//!   definitions, rather than the listing
//! - The hidden name's refusal, which reaches the caller before the contract
//!   the listed state enforces
//!
//! ## Does not own
//! - What a profile advertises (`tool_profile_tests`)
//! - What a tool's schema declares (`schema_defs` tests)
//!
//! ## Boundary
//!
//! Changes to where a definition or a contract is read from should normally
//! require changes here. Changes to what a schema declares should not.
//!
//! Keep this module focused on lookup and contract provenance.

use crate::arg_contract::contracts;
use crate::protocol::{ERR_INVALID_PARAMS, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use crate::schema::{catalog_def, catalog_defs};
use serde_json::{Value, json};
use std::sync::Arc;

/// A tool the minimal profile hides and this build still binds.
const HIDDEN: &str = "store";

/// A server state whose configuration layer is exactly `config`.
///
/// The runtime reads no process environment, so the profile in force depends on
/// the given document alone.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// The advertised tool names for one state, in listing order.
fn listed(state: &McpState) -> Vec<String> {
    let value = McpHandler::tools_list_visible(state).expect("a known profile lists its tools");
    value["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The structured content of one `tools/call`, which is where a refused call
/// places its `ok`, `code`, and `message`.
fn call(state: &Arc<McpState>, name: &str, args: &Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": args }
    });
    let response = McpHandler::new(Arc::clone(state))
        .handle_message(&request)
        .expect("a request with an id has a response");
    response["result"]["structuredContent"].clone()
}

/// The code and message of one refused call.
fn refusal_of(state: &Arc<McpState>, name: &str, args: &Value) -> (String, String) {
    let content = call(state, name, args);
    assert_eq!(content["ok"], json!(false), "{content}");
    (
        content["code"].as_str().expect("a refusal code").to_owned(),
        content["message"]
            .as_str()
            .expect("a refusal message")
            .to_owned(),
    )
}

/// The code and message of one call the handler refused before any tool ran.
///
/// The refusal is the JSON-RPC error arm, so the response carries no result at
/// all: no payload and no structured refusal.
fn rpc_refusal_of(state: &Arc<McpState>, name: &str, args: &Value) -> (i64, String) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": args }
    });
    let response = McpHandler::new(Arc::clone(state))
        .handle_message(&request)
        .expect("a request with an id has a response");
    assert!(
        response.get("result").is_none(),
        "a refused name is no result: {response}"
    );
    (
        response["error"]["code"]
            .as_i64()
            .expect("a JSON-RPC error code"),
        response["error"]["message"]
            .as_str()
            .expect("a refusal message")
            .to_owned(),
    )
}

/// The arguments each case offers `store`: nothing declared, one declared
/// property of the wrong type, and a name no schema declares. Each is refused
/// before the tool body reads anything.
fn refused_arguments() -> Vec<Value> {
    vec![
        json!({}),
        json!({"item": "text", "epistemic_kind": "fact"}),
        json!({"item": "text", "bogus": 1}),
    ]
}

#[test]
fn an_exact_lookup_resolves_a_tool_the_profile_hides() {
    let state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    assert!(
        !listed(&state).contains(&HIDDEN.to_owned()),
        "the minimal profile does not list a write tool"
    );
    let hidden = catalog_def(HIDDEN).expect("the full catalog defines the hidden tool");
    let mut from_catalog: Vec<_> = catalog_defs()
        .into_iter()
        .filter(|def| def.name == HIDDEN)
        .collect();
    assert_eq!(from_catalog.len(), 1, "one definition per name");
    assert_eq!(hidden, from_catalog.remove(0));
    assert_eq!(hidden.to_tool()["inputSchema"]["type"], json!("object"));
    assert_eq!(catalog_def("no_such_tool"), None);
}

#[test]
fn a_hidden_tool_is_refused_as_unknown_before_its_contract() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    assert!(!listed(&hidden_state).contains(&HIDDEN.to_owned()));
    assert!(listed(&listed_state).contains(&HIDDEN.to_owned()));
    assert!(
        contracts().contains_key(HIDDEN),
        "the contract exists while the listing hides the tool"
    );
    let declared = catalog_def(HIDDEN).expect("definition").required;
    assert!(
        !declared.is_empty(),
        "a tool with no required argument proves nothing"
    );
    // The visible view decides before the contract, so every argument set,
    // valid or not, gets the one refusal a hidden name gets.
    for args in refused_arguments() {
        assert_eq!(
            rpc_refusal_of(&hidden_state, HIDDEN, &args),
            (ERR_INVALID_PARAMS, format!("unknown tool `{HIDDEN}`")),
            "a hidden tool is refused as an unknown one: {args}"
        );
    }
}

#[test]
fn a_hidden_tool_refuses_an_invalid_argument_as_an_unknown_tool() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    // The listed state enforces the contract, so the same arguments answer two
    // different refusals: the fault while listed, the hidden name while not.
    let missing = refusal_of(&listed_state, HIDDEN, &json!({}));
    assert_eq!(missing.0, "invalid_argument", "{missing:?}");
    assert_eq!(missing.1, "item is required");
    assert_eq!(
        rpc_refusal_of(&hidden_state, HIDDEN, &json!({})),
        (ERR_INVALID_PARAMS, format!("unknown tool `{HIDDEN}`")),
        "a hidden tool never reaches its contract"
    );
    let mistyped = refusal_of(
        &listed_state,
        HIDDEN,
        &json!({"item": "text", "epistemic_kind": "fact"}),
    );
    assert_eq!(mistyped.0, "invalid_type", "{mistyped:?}");
    assert_eq!(mistyped.1, "item must be an object");
    assert_eq!(
        rpc_refusal_of(
            &hidden_state,
            HIDDEN,
            &json!({"item": "text", "epistemic_kind": "fact"})
        ),
        (ERR_INVALID_PARAMS, format!("unknown tool `{HIDDEN}`")),
        "the typed fault the listed state refuses is the hidden name's refusal"
    );
}

#[test]
fn the_contract_set_is_the_bound_catalog_plus_the_published_discovery_definitions() {
    let before: Vec<String> = {
        let mut names: Vec<String> = contracts().keys().cloned().collect();
        names.sort_unstable();
        names
    };
    let mut bound: Vec<String> = crate::bound_tools()
        .into_iter()
        .map(str::to_owned)
        .collect();
    bound.sort_unstable();
    // The published discovery definitions carry a contract too, because the
    // `tool_invoke` wrapper and every target it is asked for are checked by the
    // same rules. They are a flag-controlled surface, not business tools, so
    // they are named here beside the bound catalog rather than inside it.
    let mut discovery: Vec<String> = crate::discovery_tool_defs::definitions()["tools"]
        .as_array()
        .expect("the document declares a tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    discovery.sort_unstable();
    let mut expected = bound.clone();
    expected.extend(discovery.clone());
    expected.sort_unstable();
    assert_eq!(
        before, expected,
        "every bound tool and every published discovery tool has a contract"
    );

    let minimal = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let all = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    let minimal_names = listed(&minimal);
    let all_names = listed(&all);
    assert_ne!(
        minimal_names, all_names,
        "the two profiles list differently"
    );
    assert!(
        minimal_names.len() < bound.len(),
        "a profile narrows the listing and not the contract set"
    );
    assert_eq!(before.len(), bound.len() + discovery.len());
    assert_eq!(
        bound.len(),
        McpHandler::tools_list()["tools"]
            .as_array()
            .expect("tools")
            .len(),
        "the listing is the bound catalog, and no discovery tool is bound"
    );
}
