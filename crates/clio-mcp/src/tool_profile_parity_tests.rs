// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin dispatch eligibility to the
//! bound set rather than to the advertised listing.
//!
//! Every catalog tool is called through `tools/call` while the selected profile
//! omits it, and the answer is compared with the same call while listed. A call
//! may stop at the dispatch gate, the argument contract, a confirmation,
//! authorization, or inside the tool; any of those refusals is a valid outcome
//! that has to match, and handler reach is proved for one hidden tool, not all.
//!
//! ## Owns
//! - One hidden tool passing the dispatch gate and running its body
//! - The same result value for a valid, authorized call in both listings
//! - Every bound tool, called once while hidden, under the first narrow preset
//!   that hides it, answering what it answers when listed
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

/// The full bound set the dispatcher reads, which the `all` preset advertises sorted.
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
    let _ = state.replace_tool_view();

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

/// Tools whose payload reports server-side state rather than the caller's
/// arguments, so it can differ across a clock tick or between two equivalent fresh
/// states: a second-resolution timestamp (`hygiene_log_list`, `sync_key_status`,
/// `verify`, `diagnose`, `doctor`), the plan id `doctor` allocates, the
/// configuration carrying the profile under test (`config_get`), or the device id
/// a fresh store mints (`sync_status`). Each entry is still called while hidden and
/// still has to agree on its outcome (JSON-RPC error, `isError`, `ok`, code,
/// message); only the byte payload comparison is skipped.
const SERVER_STATE_PAYLOADS: &[&str] = &[
    "config_get",
    "diagnose",
    "doctor",
    "hygiene_log_list",
    "sync_key_status",
    "sync_status",
    "verify",
];

#[test]
fn every_catalog_tool_answers_when_hidden() {
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
    // What the call says about itself, apart from the payload: a JSON-RPC
    // refusal, and the tool outcome behind the envelope.
    let outcome = |response: &Value| {
        let content = &response["result"]["structuredContent"];
        json!([
            response.get("error").cloned().unwrap_or(Value::Null),
            response["result"]["isError"],
            content["ok"],
            content["code"],
            content["message"],
        ])
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
    for tool in SERVER_STATE_PAYLOADS {
        assert!(names.contains(tool), "`{tool}` is a bound tool");
    }
    for name in names {
        let hidden_by = ["minimal", "coding", "ops"]
            .into_iter()
            .find(|preset| {
                set_profile(&hidden_state, preset);
                !listed(&hidden_state).contains(&name.to_owned())
            })
            .unwrap_or_else(|| panic!("no preset hides the bound tool `{name}`"));
        assert!(
            listed(&visible_state).contains(&name.to_owned()),
            "the all preset must list `{name}`"
        );
        // The arguments come from the tool's own published schema and are the
        // same on both sides, so the profile is the only difference.
        let args = minimal_args(
            &crate::schema::catalog_def(name)
                .expect("a bound tool carries a catalog definition")
                .to_tool(),
        );
        let hidden = call(&hidden_state, name, &args);
        let visible = call(&visible_state, name, &args);
        // The gate refuses an unbound name with `-32602` naming it unknown: the
        // code selects that refusal, and the message only corroborates it.
        let error = &hidden["error"];
        let refused_at_gate = error["code"] == json!(ERR_INVALID_PARAMS)
            && error["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("unknown tool `{name}`"));
        assert!(
            !refused_at_gate,
            "`{name}` was refused at the dispatch gate for being unlisted by {hidden_by}: {hidden}"
        );
        assert_eq!(
            outcome(&hidden),
            outcome(&visible),
            "`{name}` answered a different outcome while hidden by {hidden_by}"
        );
        if !SERVER_STATE_PAYLOADS.contains(&name) {
            assert_eq!(
                hidden, visible,
                "`{name}` must answer the same payload whether its profile lists it or hides it"
            );
        }
    }
}
