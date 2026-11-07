// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the call tally at the shared execute path: a
//! `tools/call` that runs a tool is counted once under that tool's name, a call
//! the bound set or the argument contract refuses is counted nowhere, and the
//! response bytes are the ones the same call path produced before the tally
//! existed.
//!
//! ## Owns
//! - `a_direct_call_records_its_tool_once`
//! - `a_refused_unknown_tool_records_nothing`
//! - `a_call_rejected_by_the_argument_contract_records_nothing`
//! - `two_calls_record_two`
//! - `a_result_is_unchanged_by_accounting`
//!
//! ## Does not own
//! - The tally's own arithmetic (see `call_accounting_tests`)
//! - Which tools exist, what they accept, or what they return (see each tool's
//!   own test module)
//!
//! ## Boundary
//!
//! Changes to when a call is counted should normally require changes here.
//! Changes to a tool's result should not: this module compares a response with
//! the same path, not with a pinned payload.
//!
//! Keep this module focused on the tally's placement in the execute path.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A state over the default in-memory backend, and a handler on it.
fn handler() -> (McpHandler, Arc<McpState>) {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let handler = McpHandler::new(Arc::clone(&state));
    (handler, state)
}

/// Send one `tools/call` and return the whole response, so a gate refusal (a
/// JSON-RPC `error`) and a tool refusal (a structured result) stay distinct.
fn call(handler: &McpHandler, name: &str, args: &Value) -> Value {
    handler
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }))
        .expect("a request with an id has a response")
}

#[test]
fn a_direct_call_records_its_tool_once() {
    let (handler, state) = handler();
    let response = call(&handler, "config_profiles", &json!({}));

    assert_eq!(response["result"]["isError"], json!(false));
    assert_eq!(
        state.calls.snapshot(),
        BTreeMap::from([("config_profiles".to_owned(), 1)])
    );
}

#[test]
fn a_refused_unknown_tool_records_nothing() {
    let (handler, state) = handler();
    let response = call(&handler, "no_such_tool", &json!({}));

    assert_eq!(response["error"]["code"], json!(ERR_INVALID_PARAMS));
    assert_eq!(
        response["error"]["message"],
        json!("unknown tool `no_such_tool`")
    );
    assert!(
        state.calls.snapshot().is_empty(),
        "a refused name never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn a_call_rejected_by_the_argument_contract_records_nothing() {
    let (handler, state) = handler();
    let response = call(
        &handler,
        "store",
        &json!({"text": "hello", "epistemic_kind": "fact"}),
    );

    assert_eq!(response["result"]["isError"], json!(true));
    let payload = &response["result"]["structuredContent"];
    assert_eq!(payload["code"], json!("invalid_argument"));
    assert_eq!(
        payload["message"],
        json!("unknown argument `text` for `store`; tools/list names the accepted arguments")
    );
    assert!(
        state.calls.snapshot().is_empty(),
        "a rejected call never ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn two_calls_record_two() {
    let (handler, state) = handler();
    let _ = call(&handler, "config_profiles", &json!({}));
    let _ = call(&handler, "config_profiles", &json!({}));

    assert_eq!(state.calls.snapshot().get("config_profiles"), Some(&2));
}

#[test]
fn a_result_is_unchanged_by_accounting() {
    let (handler, state) = handler();
    let args = json!({});
    let has_schema = crate::schema_output_defs::output_schema("config_profiles").is_some();
    let dispatched = crate::write_tools::dispatch(&state, "config_profiles", &args);
    let expected = tool_call_envelope(&dispatched, has_schema);

    let response = call(&handler, "config_profiles", &args);

    assert_eq!(response["result"], expected);
}
