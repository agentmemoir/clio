// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that hold `tool_invoke` to its
//! published contract: an invoked target answers exactly as a direct call does,
//! a target that cannot be invoked is refused and recorded nowhere, and the
//! wrapper is reachable only where the configuration turns discovery on.
//!
//! ## Owns
//! - `an_invoked_target_answers_as_the_direct_call_does`
//! - `an_invocation_forwards_the_callers_arguments_to_the_target`
//! - `an_absent_arguments_member_answers_as_an_explicit_empty_object`
//! - `a_discovery_target_is_refused_and_records_nothing`
//! - `an_unbound_target_is_refused_and_named`
//! - `an_invalid_target_argument_names_the_target_and_the_field`
//! - `an_unknown_target_argument_names_the_target_once`
//! - `a_wrapper_failure_names_the_wrapper_and_the_field`
//! - `one_invocation_records_only_the_target`
//! - `the_execute_path_refuses_a_discovery_name_by_any_route`
//! - `the_wrapper_is_refused_as_an_unknown_tool_until_discovery_is_on`
//!
//! ## Does not own
//! - What the target tool returns (see each tool's own test module)
//! - The tally's own arithmetic (see `call_accounting_tests`)
//! - The declaration of the three discovery tools (see
//!   `discovery_tool_defs_tests`)
//! - Whether a published example matches the handler (see
//!   `catalog_invoke_examples_tests`)
//!
//! ## Boundary
//!
//! Changes to what the wrapper refuses, or to what it answers with, should
//! normally require changes here. Changes to a business tool's parameters or
//! payload should not: the wrapper is compared against the direct call path, not
//! against a pinned payload.
//!
//! Every test opens its own state over the in-memory backend. The targets are
//! configuration reads, apart from the one test that writes an item through the
//! store at a pinned id and timestamp and reads it back by that id, so the
//! module opens no path, holds no clock, and depends on no environment.

use super::*;
use crate::protocol::ERR_INVALID_PARAMS;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The read tool these tests invoke: it answers from the configuration document
/// alone, so two calls with the same arguments answer the same bytes.
const TARGET: &str = "config_profiles";

/// The read tool whose declared `item_id` decides its answer: the item stored
/// under that id, or a not-found naming the id it was given.
const BY_ID: &str = "get";

/// The item the forwarding test writes and reads back. Both halves name the same
/// pinned id and the write pins its timestamp, so nothing here is minted.
const STORED_ITEM: &str = "itm-invoke-forwarding";

/// The three names the embedded document declares, which the wrapper refuses.
const DISCOVERY: [&str; 3] = ["tool_search", "tool_describe", "tool_invoke"];

/// The `store` arguments that write [`STORED_ITEM`] into the default bank.
fn stored_item() -> Value {
    json!({
        "item": {
            "id": STORED_ITEM,
            "subject_id": STORED_ITEM,
            "gist": "prefers terse answers",
            "snapshot": {
                "entity": "terse answers",
                "amount": 1,
                "date": "2024-01-01"
            }
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
        "timestamp": "2024-01-01T00:00:00Z"
    })
}

/// A state whose configuration layer is exactly `config`, and a handler on it.
fn handler_with(config: &Value) -> (McpHandler, Arc<McpState>) {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), config).expect("state");
    let handler = McpHandler::new(Arc::clone(&state));
    (handler, state)
}

/// A state whose configuration turns discovery on, so the wrapper is reachable.
pub(super) fn handler() -> (McpHandler, Arc<McpState>) {
    handler_with(&json!({ "mcp": { "discovery": true } }))
}

/// Send one `tools/call` with the given params and return the whole response, so
/// a gate refusal (a JSON-RPC `error`) and a tool refusal (a structured result)
/// stay distinct.
#[allow(clippy::needless_pass_by_value)]
fn request(handler: &McpHandler, params: Value) -> Value {
    handler
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params
        }))
        .expect("a request with an id has a response")
}

/// Send one `tools/call` naming a tool and its arguments.
pub(super) fn call(handler: &McpHandler, name: &str, args: &Value) -> Value {
    request(handler, json!({ "name": name, "arguments": args }))
}

/// The payload a `tools/call` result carries, whichever block holds it: the
/// `structuredContent` of a tool with a declared output schema, or the text
/// block of one without.
pub(super) fn payload(response: &Value) -> Value {
    let result = &response["result"];
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    let text = result["content"][0]["text"]
        .as_str()
        .expect("a result carries a text block");
    serde_json::from_str(text).expect("the text block holds the payload")
}

/// The code and message of one refused `tool_invoke` call.
pub(super) fn refusal_of(response: &Value) -> (String, String) {
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    let payload = payload(response);
    assert_eq!(payload["ok"], json!(false), "{payload}");
    (
        payload["code"].as_str().expect("a refusal code").to_owned(),
        payload["message"]
            .as_str()
            .expect("a refusal message")
            .to_owned(),
    )
}

#[test]
fn an_invoked_target_answers_as_the_direct_call_does() {
    let (handler, _state) = handler();
    let args = json!({});
    let direct = call(&handler, TARGET, &args);
    assert_eq!(direct["result"]["isError"], json!(false), "{direct}");

    let invoked = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": TARGET, "arguments": args }),
    );

    assert_eq!(
        invoked, direct,
        "an invoked call answers exactly what the direct call answers"
    );
}

#[test]
fn an_invocation_forwards_the_callers_arguments_to_the_target() {
    let (handler, _state) = handler();
    let write = call(&handler, "store", &stored_item());
    assert_eq!(write["result"]["isError"], json!(false), "{write}");

    // `get` answers from the id its argument names, so the argument decides the
    // response: the stored item, or a not-found carrying whatever id arrived.
    let args = json!({ "item_id": STORED_ITEM });
    let direct = call(&handler, BY_ID, &args);
    assert_eq!(direct["result"]["isError"], json!(false), "{direct}");

    let invoked = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": BY_ID, "arguments": args }),
    );
    assert_eq!(
        invoked, direct,
        "the forwarded call answers exactly what the direct call answers"
    );
    assert_eq!(
        payload(&invoked)["id"],
        json!(STORED_ITEM),
        "the answer is the item the caller's argument named: {invoked}"
    );

    let without = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": BY_ID, "arguments": {} }),
    );
    assert_ne!(
        without, invoked,
        "an invocation carrying no argument is not this answer, so a wrapper \
         that dropped the caller's arguments fails here"
    );
    let elsewhere = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": BY_ID, "arguments": { "item_id": "itm-not-stored" } }),
    );
    let (code, message) = refusal_of(&elsewhere);
    assert_eq!(code, "item_not_found", "{elsewhere}");
    assert!(
        message.contains("itm-not-stored"),
        "the id the caller's argument named is the one reported: {message}"
    );
}

#[test]
fn an_absent_arguments_member_answers_as_an_explicit_empty_object() {
    let (handler, _state) = handler();
    let absent = request(
        &handler,
        json!({ "name": TOOL_INVOKE, "arguments": { "name": TARGET } }),
    );
    let explicit = request(
        &handler,
        json!({ "name": TOOL_INVOKE, "arguments": { "name": TARGET, "arguments": {} } }),
    );

    assert_eq!(absent["result"]["isError"], json!(false), "{absent}");
    assert_eq!(
        absent, explicit,
        "the absent-arguments default and an explicit empty object answer alike"
    );
}

#[test]
fn a_discovery_target_is_refused_and_records_nothing() {
    let (handler, state) = handler();
    for target in DISCOVERY {
        let response = call(&handler, TOOL_INVOKE, &json!({ "name": target }));
        let (code, message) = refusal_of(&response);
        assert_eq!(code, "invalid_argument", "{target}: {response}");
        assert!(
            message.contains(target),
            "{target}: the refusal names the target: {message}"
        );
        assert!(
            state.calls.snapshot().is_empty(),
            "{target}: a refused target never ran: {:?}",
            state.calls.snapshot()
        );
    }
}

#[test]
fn an_unbound_target_is_refused_and_named() {
    let (handler, state) = handler();
    let response = call(&handler, TOOL_INVOKE, &json!({ "name": "no_such_tool" }));

    // A name no view advertises gets the refusal a direct call gets, so the
    // wrapper cannot be used to tell an unbound name from a hidden one.
    assert_eq!(
        response["error"]["code"],
        json!(ERR_INVALID_PARAMS),
        "{response}"
    );
    assert_eq!(
        response["error"]["message"],
        json!("unknown tool `no_such_tool`"),
        "the refusal names the target"
    );
    assert!(
        state.calls.snapshot().is_empty(),
        "a refused target never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn an_invalid_target_argument_names_the_target_and_the_field() {
    let (handler, state) = handler();
    // A missing required argument is the fault whose message does not already
    // carry the tool name, so it is the one that proves the wrapper adds it.
    let response = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": "store", "arguments": {} }),
    );
    let (code, message) = refusal_of(&response);

    assert_eq!(code, "invalid_argument", "{response}");
    assert!(message.contains("store"), "names the target: {message}");
    assert!(message.contains("item"), "names the field: {message}");
    assert!(
        state.calls.snapshot().is_empty(),
        "a refused target never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn an_unknown_target_argument_names_the_target_once() {
    let (handler, state) = handler();
    // The contract's own message for an undeclared argument already names the
    // tool, so the wrapper adds nothing and the target appears once.
    let response = call(
        &handler,
        TOOL_INVOKE,
        &json!({
            "name": "store",
            "arguments": { "item": {}, "epistemic_kind": "fact", "bogus": 1 }
        }),
    );
    let (code, message) = refusal_of(&response);

    assert_eq!(code, "invalid_argument", "{response}");
    assert_eq!(
        message.matches("store").count(),
        1,
        "the target is named exactly once: {message}"
    );
    assert!(message.contains("bogus"), "names the field: {message}");
    assert!(
        state.calls.snapshot().is_empty(),
        "a refused target never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn a_wrapper_failure_names_the_wrapper_and_the_field() {
    let (handler, state) = handler();
    let cases = [
        (json!({}), "invalid_argument", "name"),
        (json!({ "name": 7 }), "invalid_type", "name"),
        (json!({ "name": "" }), "invalid_argument", "name"),
        (
            json!({ "name": TARGET, "arguments": "not-an-object" }),
            "invalid_type",
            "arguments",
        ),
    ];
    // Each case carries the code the shared validator gives that fault, so the
    // wrapper proves it keeps the code rather than replacing it.
    for (wrapper, expected, field) in cases {
        let response = call(&handler, TOOL_INVOKE, &wrapper);
        let (code, message) = refusal_of(&response);
        assert_eq!(code, expected, "{wrapper}: {response}");
        assert!(
            message.contains(TOOL_INVOKE),
            "{wrapper}: the refusal names the wrapper: {message}"
        );
        assert!(
            message.contains(field),
            "{wrapper}: the refusal names the field `{field}`: {message}"
        );
        assert!(
            state.calls.snapshot().is_empty(),
            "{wrapper}: a refused wrapper never ran: {:?}",
            state.calls.snapshot()
        );
    }
}

#[test]
fn one_invocation_records_only_the_target() {
    let (handler, state) = handler();
    let response = call(
        &handler,
        TOOL_INVOKE,
        &json!({ "name": TARGET, "arguments": {} }),
    );

    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    assert_eq!(
        state.calls.snapshot(),
        BTreeMap::from([(TARGET.to_owned(), 1)]),
        "the inner tool is counted once, and the wrapper is counted nowhere"
    );
}

#[test]
fn the_execute_path_refuses_a_discovery_name_by_any_route() {
    let (_handler, state) = handler();
    for target in DISCOVERY {
        let err = McpHandler::execute_tool_call(&state, target, &json!({}))
            .expect_err("the shared execute path refuses a discovery name");
        assert_eq!(err.code, ERR_INVALID_PARAMS, "{target}");
        assert!(
            err.message.contains(target),
            "{target}: the refusal names the tool: {}",
            err.message
        );
    }
    assert!(
        state.calls.snapshot().is_empty(),
        "a refused name never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn the_wrapper_is_refused_as_an_unknown_tool_until_discovery_is_on() {
    let (off, off_state) = handler_with(&json!({}));
    let refused = call(&off, TOOL_INVOKE, &json!({ "name": TARGET }));
    assert_eq!(refused["error"]["code"], json!(ERR_INVALID_PARAMS));
    assert_eq!(
        refused["error"]["message"],
        json!("unknown tool `tool_invoke`")
    );
    assert!(off_state.calls.snapshot().is_empty());

    let (on, on_state) = handler();
    let served = call(
        &on,
        TOOL_INVOKE,
        &json!({ "name": TARGET, "arguments": {} }),
    );
    assert_eq!(served["result"]["isError"], json!(false), "{served}");
    assert_eq!(
        on_state.calls.snapshot(),
        BTreeMap::from([(TARGET.to_owned(), 1)])
    );
}
