// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin dispatch eligibility to the
//! bound set rather than to the advertised listing, so a tool the selected
//! profile hides answers exactly what it answers when listed.
//!
//! ## Owns
//! - A hidden tool passing the dispatch gate and running its body
//! - The same result value for a valid, authorized call in both listings
//! - The same code and message for an invalid call and for an unauthorized one
//! - A server-instance profile replaced through the config runtime changing the
//!   next listing, with no request method selecting a profile
//!
//! ## Does not own
//! - What `tools/list` advertises (`tool_profile_tests`)
//! - Where a definition or a contract is read from (`catalog_lookup_tests`)
//! - The authorization rule itself (`runtime::authorize_admin`)
//!
//! ## Boundary
//!
//! Tests here break when dispatch starts reading the visible listing, or when a
//! profile stops being derived from the server's config on each request. They
//! never change the gate, the contract, or the profile data.
//!
//! Keep this module focused on the callable/listed/hidden parity.

use crate::bound_tools;
use crate::principal::Principal;
use crate::protocol::{ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::Actor;
use serde_json::{Value, json};
use std::sync::Arc;

/// A tool outside the minimal profile that needs no store: any outcome it gives
/// comes from the gate, the contract, and its body, never from an empty database.
const HIDDEN: &str = "config_profiles";

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

/// The same state carrying one credential, so the administrative gate has an
/// actor to read.
fn bound(state: &Arc<McpState>, actor: Actor) -> Arc<McpState> {
    Arc::new(state.as_ref().clone().with_principal(Principal::new(
        format!("{}-key", actor.as_str()),
        actor,
        vec!["default".to_owned()],
    )))
}

/// Send one framed request and return its response.
fn request(state: &Arc<McpState>, message: &Value) -> Value {
    McpHandler::new(Arc::clone(state))
        .handle_message(message)
        .expect("a request with an id has a response")
}

/// Send one `tools/call` and return the whole response, so a gate refusal (a
/// JSON-RPC `error`) and a tool refusal (a structured result) stay distinct.
fn call(state: &Arc<McpState>, name: &str, args: &Value) -> Value {
    request(
        state,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }),
    )
}

/// The code and message of one refused call.
fn refusal(state: &Arc<McpState>, name: &str, args: &Value) -> (String, String) {
    let content = call(state, name, args)["result"]["structuredContent"].clone();
    assert_eq!(content["ok"], json!(false), "{content}");
    (
        content["code"].as_str().expect("a refusal code").to_owned(),
        content["message"]
            .as_str()
            .expect("a refusal message")
            .to_owned(),
    )
}

/// The tool names one `tools/list` request advertises, sorted.
fn listed(state: &Arc<McpState>) -> Vec<String> {
    let response = request(
        state,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let mut names: Vec<String> = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// The minimal preset's tool names, written out so a membership change fails
/// these tests instead of moving the expectation with the code.
const MINIMAL_TOOLS: [&str; 6] = [
    "associations",
    "compose_context",
    "graph_link",
    "graph_query",
    "intent_gate",
    "retrieve",
];

/// The minimal preset's expected listing, sorted.
fn expected_minimal() -> Vec<String> {
    MINIMAL_TOOLS.iter().map(|name| (*name).to_owned()).collect()
}

/// The full bound set the dispatcher reads, which the `all` preset advertises
/// unchanged, sorted.
fn full_catalog() -> Vec<String> {
    let mut names: Vec<String> = McpHandler::tools_list()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

#[test]
fn a_hidden_tool_passes_the_dispatch_gate() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    assert!(!listed(&hidden_state).contains(&HIDDEN.to_owned()));
    assert!(listed(&listed_state).contains(&HIDDEN.to_owned()));
    assert!(
        bound_tools().contains(&HIDDEN),
        "this build binds the tool the profile hides"
    );

    let hidden = call(&hidden_state, HIDDEN, &json!({}));
    assert!(hidden.get("error").is_none(), "{hidden}");
    assert_eq!(hidden["result"]["isError"], json!(false), "{hidden}");
    assert_eq!(
        hidden["result"]["structuredContent"]["ok"],
        json!(true),
        "the hidden tool ran its body instead of being refused at the gate: {hidden}"
    );

    // The gate's own refusal, for contrast: a name this build does not bind
    // never reaches a tool body, in either listing.
    let unbound = call(&hidden_state, "no_such_tool", &json!({}));
    assert_eq!(
        unbound["error"]["code"],
        json!(ERR_INVALID_PARAMS),
        "{unbound}"
    );
    assert_eq!(
        unbound["error"]["message"],
        json!("unknown tool `no_such_tool`"),
        "{unbound}"
    );
    assert_eq!(
        unbound["error"],
        call(&listed_state, "no_such_tool", &json!({}))["error"]
    );
}

#[test]
fn a_valid_authorized_call_answers_the_same_whether_listed_or_hidden() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    for (name, args) in [
        ("config_profiles", json!({})),
        ("ranking_env_get", json!({})),
    ] {
        assert!(
            !listed(&hidden_state).contains(&name.to_owned()),
            "premise: the minimal profile hides `{name}`"
        );
        assert!(
            listed(&listed_state).contains(&name.to_owned()),
            "premise: the all profile lists `{name}`"
        );
        let hidden = call(&hidden_state, name, &args);
        assert_eq!(
            hidden["result"],
            call(&listed_state, name, &args)["result"],
            "one outcome for `{name}` in both listings"
        );
        assert_eq!(
            hidden["result"]["structuredContent"]["ok"],
            json!(true),
            "`{name}` is valid and authorized in both listings: {hidden}"
        );
    }
}

#[test]
fn an_invalid_call_is_refused_the_same_whether_listed_or_hidden() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    for args in [
        json!({"path": 1}),
        json!({"path": "log.level", "bogus": true}),
    ] {
        let hidden = call(&hidden_state, "config_get", &args);
        let listed = call(&listed_state, "config_get", &args);
        assert_eq!(
            hidden["result"]["structuredContent"], listed["result"]["structuredContent"],
            "one refusal for {args}"
        );
        assert_eq!(hidden["result"]["isError"], json!(true), "{hidden}");
    }
    let mistyped = refusal(&hidden_state, "config_get", &json!({"path": 1}));
    assert_eq!(mistyped.0, "invalid_type", "{mistyped:?}");
    assert_eq!(mistyped.1, "path must be a string");
    assert_eq!(
        mistyped,
        refusal(&listed_state, "config_get", &json!({"path": 1}))
    );
}

#[test]
fn an_unauthorized_call_is_refused_the_same_whether_listed_or_hidden() {
    let hidden_state = bound(
        &state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#),
        Actor::Agent,
    );
    let listed_state = bound(
        &state_with(r#"{"mcp": {"tool_profile": "all"}}"#),
        Actor::Agent,
    );
    let args = json!({"path": "log.level", "value": "info"});
    assert!(
        !listed(&hidden_state).contains(&"config_set".to_owned()),
        "premise: the minimal profile hides the administrative tool"
    );
    let hidden = refusal(&hidden_state, "config_set", &args);
    assert_eq!(hidden.0, "forbidden", "{hidden:?}");
    assert!(hidden.1.contains("not an administrator"), "{}", hidden.1);
    assert_eq!(
        hidden,
        refusal(&listed_state, "config_set", &args),
        "the refusal names the credential, not the listing"
    );

    // Control: the same call with an administrative credential is accepted, so
    // the refusal above came from authorization rather than from the arguments.
    for state in [
        bound(
            &state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#),
            Actor::Harness,
        ),
        bound(
            &state_with(r#"{"mcp": {"tool_profile": "all"}}"#),
            Actor::Harness,
        ),
    ] {
        let accepted = call(&state, "config_set", &args);
        assert_eq!(
            accepted["result"]["structuredContent"]["ok"],
            json!(true),
            "{accepted}"
        );
    }
}

#[test]
fn a_default_state_keeps_the_minimal_listing_next_to_a_wider_one() {
    let default_state = state_with("{}");
    let all_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    assert_eq!(listed(&default_state), expected_minimal());
    assert_eq!(listed(&all_state), full_catalog());

    // A request against one server instance leaves the other instance's
    // profile exactly as its own configuration selects it.
    let _ = call(&all_state, HIDDEN, &json!({}));
    assert_eq!(listed(&default_state), expected_minimal());
    assert_eq!(listed(&all_state), full_catalog());
}

#[test]
fn replacing_the_server_config_changes_the_next_listing() {
    let dir = std::env::temp_dir().join(format!("clio-tool-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("deployment.json");
    std::fs::write(&path, r#"{"mcp": {"tool_profile": "minimal"}}"#).expect("write config");
    let state = McpState::open_with_runtime(
        &McpOpenOptions::default(),
        clio_config::Runtime::with_env_and_deployment_file(&|_| None, path.clone()),
    )
    .expect("state");

    let before = listed(&state);
    assert_eq!(before, expected_minimal());
    assert!(!before.contains(&"store".to_owned()));

    // The runtime behind `state.config` is server-owned: the host replaces the
    // document it resolves from, exactly as it installed the first one.
    std::fs::write(&path, r#"{"mcp": {"tool_profile": "all"}}"#).expect("replace config");
    {
        let mut runtime = state.config.lock().expect("config runtime mutex poisoned");
        runtime
            .reload_deployment_overlay()
            .expect("the replaced document loads");
    }

    let after = listed(&state);
    assert_eq!(after, full_catalog());
    assert!(after.contains(&"store".to_owned()), "{after:?}");

    // The listing followed the configuration alone: no request method selects a
    // profile, so a client cannot switch the set out from under the server.
    let unselected = request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/profile",
            "params": { "name": "all" }
        }),
    );
    assert_eq!(
        unselected["error"]["code"],
        json!(ERR_METHOD_NOT_FOUND),
        "{unselected}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
