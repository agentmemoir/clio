// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that keep dispatch eligibility
//! equal to the advertised listing: a tool the effective profile hides is
//! refused at the `tools/call` gate, in the same `-32602` shape an unknown
//! name gets, before its argument contract or its body is reached.
//!
//! Every catalog tool is called through `tools/call` while the selected preset
//! omits it, and the answer has to be that gate refusal. The same call while
//! listed reaches the tool, so a listed tool keeps behaving exactly as it did
//! before the gate started reading the view.
//!
//! ## Owns
//! - A hidden bound tool being refused at the dispatch gate and never running
//! - A listed tool reaching its body, and its refusals staying its own
//! - Every bound tool, called once while hidden, refused by the gate
//! - A listed tool being refused by its contract or by authorization instead
//! - A server-instance profile replaced through the config runtime changing the
//!   next listing, with no request method selecting a profile
//!
//! ## Does not own
//! - What `tools/list` advertises (`tool_profile_tests`)
//! - The gate's own tests for presets and selections
//!   (`protocol_dispatch_gate_tests`)
//! - Where a definition or a contract is read from (`catalog_lookup_tests`)
//! - The authorization rule itself (`runtime::authorize_admin`)
//!
//! ## Boundary
//!
//! Tests here break when dispatch stops reading the visible listing, or when a
//! profile stops being derived from the server's config on each request. They
//! never change the gate, the contract, or the profile data.
//!
//! Keep this module focused on the dispatched/listed/hidden parity.

use super::protocol_nfr7_tests::minimal_args;
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
/// The runtime reads no environment, so the profile in force is the document's alone.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// The same state carrying one credential, so the administrative gate has an actor.
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

/// Send one `tools/call` and return its whole response, keeping gate and tool refusals apart.
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

/// The code and message of the gate's refusal of one call.
///
/// A tool body's own refusal arrives in `structuredContent`; a name the view
/// leaves out arrives as a JSON-RPC error instead, which is what this reads.
fn gate_refusal(state: &Arc<McpState>, name: &str, args: &Value) -> (i64, String) {
    let response = call(state, name, args);
    assert!(
        response.get("result").is_none(),
        "`{name}` must be refused before a result is shaped: {response}"
    );
    let error = &response["error"];
    assert_eq!(error["code"], json!(ERR_INVALID_PARAMS), "{response}");
    (
        error["code"].as_i64().expect("a refusal code"),
        error["message"]
            .as_str()
            .expect("a refusal message")
            .to_owned(),
    )
}

/// The code and message of one refusal a listed tool gave from its own body.
fn tool_refusal(state: &Arc<McpState>, name: &str, args: &Value) -> (String, String) {
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

/// The tool names one `tools/list` request advertises, in one full page.
fn listed(state: &Arc<McpState>) -> Vec<String> {
    let response = request(
        state,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list",
                "params": {"_meta": {"com.agentmemoir/pageSize": 100}}}),
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

/// The minimal preset's tool names, so a membership change fails these tests.
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
    MINIMAL_TOOLS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// The full bound set the argument contract reads, which `all` advertises sorted.
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
fn a_hidden_bound_tool_is_refused_at_the_dispatch_gate() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    assert!(!listed(&hidden_state).contains(&HIDDEN.to_owned()));
    assert!(listed(&listed_state).contains(&HIDDEN.to_owned()));
    assert!(
        bound_tools().contains(&HIDDEN),
        "this build binds the tool the profile hides"
    );

    let (code, message) = gate_refusal(&hidden_state, HIDDEN, &json!({}));
    assert_eq!(code, -32602, "the gate answers the invalid-params code");
    assert_eq!(message, format!("unknown tool `{HIDDEN}`"));
    assert!(
        hidden_state.calls.snapshot().is_empty(),
        "a tool the gate refuses never ran: {:?}",
        hidden_state.calls.snapshot()
    );

    // The other half of the contract: listed, the same call reaches the body.
    let listed_response = call(&listed_state, HIDDEN, &json!({}));
    assert_eq!(listed_response["result"]["isError"], json!(false));
    assert_eq!(
        listed_response["result"]["structuredContent"]["ok"],
        json!(true),
        "a listed tool still runs: {listed_response}"
    );
    assert_eq!(
        listed_state.calls.snapshot(),
        std::collections::BTreeMap::from([(HIDDEN.to_owned(), 1)])
    );
}

#[test]
fn a_valid_call_is_listed_and_hidden_only_by_the_gate() {
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
        let accepted = call(&listed_state, name, &args);
        assert_eq!(
            accepted["result"]["structuredContent"]["ok"],
            json!(true),
            "`{name}` is valid and authorized when listed: {accepted}"
        );
        assert_eq!(
            gate_refusal(&hidden_state, name, &args),
            (-32602, format!("unknown tool `{name}`")),
            "the listing alone decides whether `{name}` reaches its body"
        );
    }
}

#[test]
fn an_invalid_call_is_refused_by_its_contract_only_when_listed() {
    let hidden_state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let listed_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    for args in [
        json!({"path": 1}),
        json!({"path": "log.level", "bogus": true}),
    ] {
        let listed = call(&listed_state, "config_get", &args);
        assert_eq!(
            listed["result"]["isError"],
            json!(true),
            "the listed tool still inspects its arguments: {listed}"
        );
        let (code, message) = gate_refusal(&hidden_state, "config_get", &args);
        assert_eq!(
            (code, message.as_str()),
            (-32602, "unknown tool `config_get`")
        );
    }
    let mistyped = tool_refusal(&listed_state, "config_get", &json!({"path": 1}));
    assert_eq!(mistyped.0, "invalid_type", "{mistyped:?}");
    assert_eq!(mistyped.1, "path must be a string");
    // The gate ran before the contract: the hidden call never named a type.
    let (_, hidden_message) = gate_refusal(&hidden_state, "config_get", &json!({"path": 1}));
    assert!(
        !hidden_message.contains("path must be a string"),
        "the hidden call reached its argument contract: {hidden_message}"
    );
}

#[test]
fn an_unauthorized_call_is_refused_at_the_gate_when_hidden() {
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
    let refusal = tool_refusal(&listed_state, "config_set", &args);
    assert_eq!(refusal.0, "forbidden", "{refusal:?}");
    assert!(refusal.1.contains("not an administrator"), "{}", refusal.1);

    // The gate answers first: an administrative credential does not make a
    // hidden name reachable, so it is not the listing that named the actor.
    for state in [
        bound(
            &state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#),
            Actor::Harness,
        ),
        hidden_state,
    ] {
        assert_eq!(
            gate_refusal(&state, "config_set", &args),
            (-32602, "unknown tool `config_set`".to_owned())
        );
    }

    // Control: the same call with an administrative credential on the wider
    // profile is accepted, so the refusal above came from authorization.
    let accepted = call(
        &bound(
            &state_with(r#"{"mcp": {"tool_profile": "all"}}"#),
            Actor::Harness,
        ),
        "config_set",
        &args,
    );
    assert_eq!(
        accepted["result"]["structuredContent"]["ok"],
        json!(true),
        "{accepted}"
    );
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
    assert_eq!(
        gate_refusal(&state, "store", &json!({})).1,
        "unknown tool `store`"
    );

    // The runtime behind `state.config` is server-owned: the host replaces the
    // document it resolves from, exactly as it installed the first one.
    std::fs::write(&path, r#"{"mcp": {"tool_profile": "all"}}"#).expect("replace config");
    {
        let mut runtime = state.config.lock().expect("config runtime mutex poisoned");
        runtime
            .reload_deployment_overlay()
            .expect("the replaced document loads");
    }
    let _ = state.replace_tool_view();

    let after = listed(&state);
    assert_eq!(after, full_catalog());
    assert!(after.contains(&"store".to_owned()), "{after:?}");
    // The gate followed the same view: the name it refused now dispatches.
    assert!(
        call(&state, "store", &json!({})).get("error").is_none(),
        "the widened profile dispatches the name it now lists"
    );

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

#[test]
fn every_catalog_tool_is_refused_at_the_gate_when_hidden() {
    let hidden_state = state_with("{}");
    let visible_state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    let set_profile = |state: &Arc<McpState>, preset: &str| {
        let mut runtime = state.config.lock().expect("config runtime mutex poisoned");
        let layer = format!(r#"{{"mcp": {{"tool_profile": "{preset}"}}}}"#);
        runtime
            .load_file_json(&layer)
            .expect("a valid config layer");
        // The seam reads the same runtime, so the guard is gone by now.
        drop(runtime);
        let _ = state.replace_tool_view();
    };
    // The bound set must be the catalog this test claims to exhaust, compared by
    // name: a same-length swap would otherwise skip a tool and still pass.
    let mut names = bound_tools();
    names.sort_unstable();
    let mut catalog: Vec<&str> = crate::schema::catalog_defs()
        .iter()
        .map(|def| def.name)
        .collect();
    catalog.sort_unstable();
    assert_eq!(names, catalog, "the loop must call every catalog tool");
    for name in names {
        let hidden_by = ["minimal", "coding", "ops"]
            .into_iter()
            .find(|preset| {
                set_profile(&hidden_state, preset);
                !listed(&hidden_state).contains(&name.to_owned())
            })
            .unwrap_or_else(|| panic!("no preset hides the bound tool `{name}`"));
        // The arguments come from the tool's own published schema and are the
        // same on both sides, so the profile is the only difference.
        let args = minimal_args(
            &crate::schema::catalog_def(name)
                .expect("a bound tool carries a catalog definition")
                .to_tool(),
        );
        let (code, message) = gate_refusal(&hidden_state, name, &args);
        assert_eq!(code, -32602, "`{name}` hidden by {hidden_by}: {message}");
        assert_eq!(message, format!("unknown tool `{name}`"));
        assert!(
            listed(&visible_state).contains(&name.to_owned()),
            "the all preset must list `{name}`"
        );
        // Listed, the same name passes the gate and answers from its body.
        assert!(
            call(&visible_state, name, &args).get("error").is_none(),
            "`{name}` must pass the gate while the all preset lists it"
        );
    }
    assert!(
        hidden_state.calls.snapshot().is_empty(),
        "no refused call reached a tool: {:?}",
        hidden_state.calls.snapshot()
    );
}
