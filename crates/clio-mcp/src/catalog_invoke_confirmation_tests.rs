// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that hold the two routes to one
//! tool — a direct `tools/call` and the same call wrapped in `tool_invoke` — to
//! a single answer for every tool the published catalog gates behind a boolean
//! `confirm` argument, and for the tools it does not gate.
//!
//! ## Owns
//! - The confirmation-gated tool set, derived from the published catalog
//! - The same normalized outcome on both routes while the profile lists the
//!   tool, with `confirm: true`, and with no `confirm` argument at all
//! - Both routes' refusal of a tool the profile hides, the wrapper's answer the
//!   direct one's, so neither route reaches a hidden confirmation or body
//! - The same non-success outcome for a well-formed call that fails a domain
//!   check rather than the argument contract
//! - Masking the identity a fresh store mints for itself and the live clock,
//!   so a comparison reads a route difference rather than ids or seconds
//!
//! ## Does not own
//! - What the confirmation gate decides, or the zero-write previews (see
//!   `mutator_tools`, `subject_request_tools`, `retention_tools`)
//! - The wrapper's own argument checking and refusals (see `catalog_invoke_tests`)
//! - Profile membership, and the listed/hidden comparison over every tool (see
//!   `tool_profile_parity_tests`)
//!
//! ## Boundary
//!
//! Changes to how a confirmation is declared, read, or refused should normally
//! require changes here. Changes to a business tool's payload should not: this
//! module compares the two routes against each other, never against a pinned
//! response.
//!
//! Every test opens its own in-memory state, with the shared bank surface
//! enabled so both `shared_*` tools reach their own gate rather than the
//! surface's refusal. The set under test is read from the catalog, so a tool
//! that stops declaring `confirm` falls into the ungated loop without an edit. A
//! tool that starts declaring it enters these loops from the catalog as well,
//! but its arguments have to carry it past whatever it checks before the gate:
//! that prerequisite table lives in `catalog_invoke_gate_fixture_tests`, which
//! panics on a gated tool it has no expectation for. So a newly gated tool needs
//! an edit there, and none here.

use super::TOOL_INVOKE;
use super::tests::{call, payload, refusal_of};
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The instant every derived argument carries, so a payload that echoes the
/// call's time is the same bytes on both routes and on two fresh states.
pub(super) const PINNED: &str = "2024-01-01T00:00:00Z";

/// A handler whose configuration lists `preset`, keeps the wrapper reachable,
/// and enables the shared bank surface.
///
/// The surface is on because two gated tools need it before they read
/// `confirm`: `shared_discard` is refused while the surface is off, and
/// `reindex` is refused without an explicit bank while it is off.
pub(super) fn handler_with(preset: &str) -> (McpHandler, Arc<McpState>) {
    let document = json!({ "mcp": { "discovery": true, "tool_profile": preset } });
    let state = McpState::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: true,
            ..McpOpenOptions::default()
        },
        &document,
    )
    .expect("a state over the in-memory backend");
    (McpHandler::new(Arc::clone(&state)), state)
}

/// The tool names one `tools/list` request advertises, in one full page.
fn listed(state: &Arc<McpState>) -> Vec<String> {
    let response = McpHandler::new(Arc::clone(state))
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            "params": {"_meta": {"com.agentmemoir/pageSize": 100}}
        }))
        .expect("a request with an id has a response");
    let mut names: Vec<String> = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// Every published tool whose input schema declares a boolean `confirm`
/// property, in catalog order.
///
/// The set is read from the catalog rather than written here, so a tool that
/// gains (or loses) a `confirm` property moves between the gated loops and the
/// ungated one without an edit to this module.
pub(super) fn gated_tools() -> Vec<Value> {
    crate::schema::catalog_defs()
        .iter()
        .map(crate::schema_defs::ToolDef::to_tool)
        .filter(|tool| tool["inputSchema"]["properties"]["confirm"]["type"] == json!("boolean"))
        .collect()
}

/// The names of [`gated_tools`], in catalog order.
fn gated_names() -> Vec<String> {
    gated_tools()
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// A minimal argument object the shared argument contract accepts for one
/// published tool.
///
/// Every required property carries a placeholder of its declared type, a
/// required string stays non-blank so the contract's blank-string refusal does
/// not fire, and the call's time is pinned. A tool is only compared on the
/// routes once its own contract accepts these arguments, because the wrapper
/// deliberately prefixes an argument-contract failure with the target name.
pub(super) fn contract_args(tool: &Value) -> Value {
    let schema = &tool["inputSchema"];
    let props = schema["properties"].as_object();
    let mut args = Map::new();
    for name in schema["required"].as_array().into_iter().flatten() {
        let Some(name) = name.as_str() else { continue };
        if let Some(prop) = props.and_then(|props| props.get(name)) {
            args.insert(name.to_owned(), placeholder(prop, schema));
        }
    }
    args.insert("timestamp".to_owned(), json!(PINNED));
    Value::Object(args)
}

/// The placeholder value one published property node declares: the first enum
/// value, the node a `$ref` points at, or an empty instance of its type.
fn placeholder(schema: &Value, root: &Value) -> Value {
    if let Some(values) = schema["enum"].as_array() {
        return values.first().cloned().unwrap_or(Value::Null);
    }
    if let Some(pointer) = schema["$ref"].as_str() {
        return placeholder(resolve_ref(root, pointer), root);
    }
    match schema["type"].as_str() {
        Some("object") => Value::Object(Map::new()),
        Some("array") => json!([]),
        Some("boolean") => json!(false),
        Some("number" | "integer") => json!(0),
        Some("string") => json!("x"),
        _ => Value::Null,
    }
}

/// The node one local JSON pointer inside a published schema names.
fn resolve_ref<'a>(root: &'a Value, pointer: &str) -> &'a Value {
    let body = pointer.strip_prefix('#').unwrap_or(pointer);
    let mut node = root;
    for segment in body
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
    {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        node = node.get(&segment).unwrap_or(&Value::Null);
    }
    node
}

/// One response, normalized: a JSON-RPC refusal keeps its `error`, and a tool
/// answer keeps its envelope plus the whole payload behind it.
pub(super) fn outcome(response: &Value) -> Value {
    if let Some(error) = response.get("error") {
        return json!({
            "error": error,
            "is_error": Value::Null,
            "payload": Value::Null,
        });
    }
    json!({
        "error": Value::Null,
        "is_error": response["result"]["isError"],
        "payload": payload(response),
    })
}

/// Call `name` directly and through the wrapper with the same arguments, and
/// return both normalized outcomes, each with the identities the one store they
/// share minted masked out.
fn both_routes(handler: &McpHandler, state: &McpState, name: &str, args: &Value) -> (Value, Value) {
    let direct = call(handler, name, args);
    let invoked = call(
        handler,
        TOOL_INVOKE,
        &json!({ "name": name, "arguments": args }),
    );
    (
        without_own_identity(&outcome(&direct), state),
        without_own_identity(&outcome(&invoked), state),
    )
}

/// A digit run at least this long inside a payload string marks that string as
/// a store-minted record identifier: no argument this module derives carries one.
const MINTED_DIGITS: usize = 12;

/// One route's answer with the identities its own store minted masked out.
///
/// The ungated comparison answers the same call from two fresh in-memory stores,
/// and each store mints its own device id and record ids. Masking exactly those
/// — the state's own device id, and a string carrying a long digit run — keeps
/// store identity from reading as a difference between the routes, while every
/// other byte is compared as it arrives: the envelope, each `code`, each
/// `message`, every non-string field, and every caller-supplied value.
pub(super) fn without_own_identity(value: &Value, state: &McpState) -> Value {
    match value {
        Value::String(text) if text == &state.device_id || carries_minted_id(text) => {
            json!("<minted>")
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| without_own_identity(item, state))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), without_own_identity(value, state)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Whether `text` carries one digit run of [`MINTED_DIGITS`] or more.
fn carries_minted_id(text: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_digit())
        .any(|run| run.len() >= MINTED_DIGITS)
}

#[test]
fn the_confirmation_gated_set_is_read_from_the_published_catalog() {
    let gated = gated_tools();
    let bound = crate::bound_tools();
    assert!(
        !gated.is_empty(),
        "the published catalog gates at least one tool behind `confirm`"
    );
    for tool in &gated {
        let name = tool["name"]
            .as_str()
            .expect("a published tool names itself");
        assert!(bound.contains(&name), "`{name}` is gated and bound");
        crate::arg_contract::check(name, &contract_args(tool)).unwrap_or_else(|err| {
            panic!(
                "`{name}`: the derived arguments must satisfy its own contract, got `{}`",
                err.message
            )
        });
    }
    // The ungated remainder is what the no-confirmation situation walks, so it
    // must not be empty either.
    let ungated = bound
        .iter()
        .any(|name| !gated_names().iter().any(|gated| gated.as_str() == *name));
    assert!(ungated, "some bound tools publish no `confirm` at all");
}

#[test]
fn an_unconfirmed_call_answers_alike_listed_or_hidden() {
    let gated = gated_tools();
    assert!(
        !gated.is_empty(),
        "the listed and hidden situations have a gated tool to compare"
    );
    let (listed_handler, listed_state) = handler_with("all");
    let (hidden_handler, hidden_state) = handler_with("minimal");
    let listed_tools = listed(&listed_state);
    let hidden_tools = listed(&hidden_state);
    let bound = crate::bound_tools();
    for tool in &gated {
        let name = tool["name"]
            .as_str()
            .expect("a published tool names itself");
        assert!(
            listed_tools.contains(&name.to_owned()),
            "the `all` profile lists `{name}`"
        );
        assert!(
            !hidden_tools.contains(&name.to_owned()),
            "the `minimal` profile hides `{name}`, so the hidden situation is real"
        );
        assert!(
            bound.contains(&name),
            "`{name}` stays bound while its profile hides it"
        );
        // The arguments the listed handler needs to reach its own gate. The
        // hidden handler is asked for none: no route may reach the tool there.
        let args = super::gate_fixture_tests::prepare(&listed_handler, tool);

        let (direct, invoked) = both_routes(&listed_handler, &listed_state, name, &args);
        assert_eq!(invoked, direct, "`{name}` answers alike while listed");

        // A hidden target gets the one `unknown tool` answer on both routes, so
        // the wrapper cannot reach a hidden confirmation prompt or tool body.
        let expected = json!({
            "error": { "code": -32602, "message": format!("unknown tool `{name}`") },
            "is_error": Value::Null,
            "payload": Value::Null,
        });
        let (hidden_direct, hidden_invoked) =
            both_routes(&hidden_handler, &hidden_state, name, &args);
        assert_eq!(hidden_direct, expected, "`{name}` is refused directly");
        assert_eq!(hidden_invoked, expected, "`{name}` by the wrapper");
    }
}

#[test]
fn a_confirmed_call_answers_alike_on_both_routes() {
    let gated = gated_tools();
    assert!(
        !gated.is_empty(),
        "a supplied confirmation has a gated tool to compare"
    );
    for tool in &gated {
        let name = tool["name"]
            .as_str()
            .expect("a published tool names itself");
        // Each route gets its own fresh state, so a confirmed call mutates one
        // store rather than being applied twice to the same one. The arguments
        // carry the tool past everything it checks before its gate, so the
        // answer compared here is the gate's answer.
        let (direct_handler, direct_state) = handler_with("all");
        let (invoke_handler, invoke_state) = handler_with("all");
        let mut args = super::gate_fixture_tests::prepare(&direct_handler, tool);
        let prepared_for_invoke = super::gate_fixture_tests::prepare(&invoke_handler, tool);
        assert_eq!(prepared_for_invoke, args, "`{name}` is prepared alike");
        args["confirm"] = json!(true);

        let direct = without_live_clock(outcome(&call(&direct_handler, name, &args)));
        let invoked = without_live_clock(outcome(&call(
            &invoke_handler,
            TOOL_INVOKE,
            &json!({ "name": name, "arguments": args }),
        )));
        assert_eq!(
            without_own_identity(&invoked, &invoke_state),
            without_own_identity(&direct, &direct_state),
            "`{name}` answers alike once confirmed"
        );
    }
}

/// One outcome with the live clock values dropped: two calls never agree on
/// measured latency, and a server-stamped `timestamp` is the second they ran
/// in. A caller-supplied timestamp survives, so an echo is still compared.
fn without_live_clock(mut value: Value) -> Value {
    if let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) {
        payload.remove("metrics");
        if payload
            .get("timestamp")
            .is_some_and(|stamp| stamp.as_str() != Some(PINNED))
        {
            payload.insert("timestamp".to_owned(), json!("<clock>"));
        }
    }
    value
}

#[test]
fn an_ungated_call_answers_alike_on_both_routes() {
    let gated = gated_names();
    // One state per route: the two routes see the same starting store, and each
    // tool in the loop is called once on each side.
    let (direct_handler, direct_state) = handler_with("all");
    let (invoke_handler, invoke_state) = handler_with("all");
    let mut exercised = 0_usize;
    let mut exact = 0_usize;
    for name in crate::bound_tools() {
        if gated.iter().any(|gated| gated.as_str() == name) {
            continue;
        }
        let tool = crate::schema::catalog_def(name)
            .expect("a bound tool carries a catalog definition")
            .to_tool();
        let args = contract_args(&tool);
        crate::arg_contract::check(name, &args).unwrap_or_else(|err| {
            panic!(
                "`{name}`: the derived arguments must satisfy its own contract, got `{}`",
                err.message
            )
        });
        let direct = without_live_clock(outcome(&call(&direct_handler, name, &args)));
        let invoked = without_live_clock(outcome(&call(
            &invoke_handler,
            TOOL_INVOKE,
            &json!({ "name": name, "arguments": args }),
        )));
        if invoked == direct {
            exact += 1;
        }
        assert_eq!(
            without_own_identity(&invoked, &invoke_state),
            without_own_identity(&direct, &direct_state),
            "`{name}` publishes no `confirm`, and both routes answer alike"
        );
        exercised += 1;
    }
    assert!(
        exercised > 0,
        "the ungated loop exercised at least one tool"
    );
    // The masking must stay narrow: some ungated tool answers the same content
    // on both routes once live clock values are dropped, with no identity
    // masking applied at all.
    assert!(exact > 0, "the ungated loop compares some answer unmasked");
}

#[test]
fn a_valid_call_that_fails_a_domain_check_fails_alike_on_both_routes() {
    let (handler, _state) = handler_with("all");
    // Well-formed arguments for a gated tool, naming an item no store holds: the
    // refusal comes from the domain, so the success of the call is not forced.
    let args = json!({ "item_id": "itm-not-stored", "reason": "operator request" });
    let direct = call(&handler, "discard", &args);
    let invoked = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": "discard", "arguments": args }),
    );

    assert_eq!(
        outcome(&invoked),
        outcome(&direct),
        "the domain refusal is the same on both routes"
    );
    let (code, message) = refusal_of(&direct);
    assert!(
        !["invalid_argument", "invalid_type"].contains(&code.as_str()),
        "`{code}` is a domain refusal, not an argument one: {message}"
    );
    assert_eq!(
        outcome(&direct)["payload"]["ok"],
        json!(false),
        "the call is reported as a failure rather than forced to succeed"
    );
}
