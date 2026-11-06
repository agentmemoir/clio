// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what a candidate `2026-07-28` result carries
//! beyond its payload: the one cache-hint pair on every cacheable result, the
//! absence of that pair on a tool result, the completeness of a failing tool
//! call, and the shape of an envelope that answers an error at all.
//!
//! ## Owns
//! - Every assertion that `CACHE_HINTS` reaches a result the revision caches
//! - The walk over every page of one listing, and the one hint pair it collects
//! - The claim that a `tools/call` failure is still a complete result
//! - The claim that an error envelope carries no result and no result type
//!
//! ## Does not own
//! - The hint values and the stamping itself (see `stateless`)
//! - The request shapes these tests send (see `stateless_fixtures`)
//! - The parse-error frame path, which owns its own assertion (see
//!   `protocol_tests`)
//!
//! ## Boundary
//!
//! Changes to what a cacheable result carries, or to how an error answer is
//! framed, should normally require changes here. Changes to what a request
//! means should not.
//!
//! Keep this module focused on the result fields a revision adds.

use super::{CACHE_HINTS, CANDIDATE_REVISION, dispatch};
use crate::protocol::{ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND};
use crate::runtime::McpState;
use crate::stateless_fixtures::{
    code, failing_tool_call, message, paged_list, request, required_meta, state, tool_call,
};
use serde_json::{Value, json};

fn answer(state: &McpState, msg: &Value) -> Value {
    dispatch(state, msg).expect("a request carrying an id is answered")
}

/// The hint pair one page carries, read from its result.
fn hint_pair(result: &Value) -> (Value, Value) {
    (result["ttlMs"].clone(), result["cacheScope"].clone())
}

/// An error envelope answers an error and nothing a client could read as a
/// result: no `result` member, and no result type at any depth.
fn assert_error_envelope(res: &Value) {
    assert!(res.get("error").is_some(), "carries an error: {res}");
    assert!(res.get("result").is_none(), "carries no result: {res}");
    let raw = serde_json::to_string(res).expect("an envelope serializes");
    assert!(!raw.contains("resultType"), "carries no result type: {raw}");
}

/// The revision types `resultType` on every result and reports tool failure in
/// the result's own `isError`, so a failed tool call is still a complete result.
#[test]
fn a_failing_tool_call_carries_a_complete_result_type() {
    let res = answer(
        &state(),
        &failing_tool_call(&required_meta(CANDIDATE_REVISION)),
    );
    assert!(
        res.get("error").is_none(),
        "a tool failure is not a JSON-RPC error: {res}"
    );
    let result = &res["result"];
    assert_eq!(result["isError"], json!(true), "the tool does fail: {res}");
    assert_eq!(result["resultType"], json!("complete"), "{res}");
}

/// One listing answered in several pages shares one cache scope: a page cannot
/// promise a scope the page before it did not.
#[test]
fn every_page_of_a_listing_carries_the_one_hint_pair() {
    let state = state();
    let meta = required_meta(CANDIDATE_REVISION);
    let mut pages: Vec<(Value, Value)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let res = answer(&state, &paged_list(&meta, 1, cursor.as_deref()));
        assert!(res.get("error").is_none(), "page {}: {res}", pages.len());
        let result = &res["result"];
        pages.push(hint_pair(result));
        match result.get("nextCursor").and_then(Value::as_str) {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert!(
        pages.len() > 1,
        "the walk visits more than one page, so the claim is not vacuous: {}",
        pages.len()
    );
    let first = pages.first().cloned().expect("a first page");
    assert_eq!(
        pages.iter().filter(|pair| **pair != first).count(),
        0,
        "every page carries the same hint pair: {pages:?}"
    );
    assert_eq!(
        first,
        (json!(CACHE_HINTS.ttl_ms), json!(CACHE_HINTS.scope)),
        "the pair is the module's one decision value: {first:?}"
    );
}

/// A tool result is not cacheable under this revision, so it carries neither
/// hint, while still carrying the completeness every result carries.
#[test]
fn a_complete_tool_call_result_carries_no_cache_hints() {
    let res = answer(
        &state(),
        &tool_call(&required_meta(CANDIDATE_REVISION), json!({})),
    );
    assert!(
        res.get("error").is_none(),
        "the fixture call succeeds: {res}"
    );
    let result = res["result"]
        .as_object()
        .unwrap_or_else(|| panic!("a tool result object: {res}"));
    assert_eq!(
        result.get("resultType"),
        Some(&json!("complete")),
        "the result is complete: {result:?}"
    );
    assert!(
        !result.contains_key("ttlMs"),
        "a tool result carries no ttl: {result:?}"
    );
    assert!(
        !result.contains_key("cacheScope"),
        "a tool result carries no scope: {result:?}"
    );
}

/// A parameter error is an envelope with no result in it: the request named no
/// tool this build serves, or named one it cannot read.
#[test]
fn a_parameter_error_carries_no_result_type() {
    let state = state();
    let meta = required_meta(CANDIDATE_REVISION);
    let cases = [
        (
            "absent name",
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": { "_meta": meta, "arguments": {} } }),
            "name is required",
        ),
        (
            "unknown name",
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": { "_meta": meta, "name": "no-such-tool", "arguments": {} } }),
            "unknown tool",
        ),
    ];
    for (label, msg, says) in cases {
        let res = answer(&state, &msg);
        assert_eq!(code(&res), ERR_INVALID_PARAMS, "case `{label}`: {res}");
        assert!(message(&res).contains(says), "case `{label}`: {res}");
        assert_error_envelope(&res);
    }
}

/// An unknown method answers an error envelope and never a result, so no
/// `resultType` is claimed for a request the server does not serve.
#[test]
fn a_method_error_carries_no_result_type() {
    let res = answer(
        &state(),
        &request("resources/list", &required_meta(CANDIDATE_REVISION)),
    );
    assert_eq!(code(&res), ERR_METHOD_NOT_FOUND, "{res}");
    assert!(message(&res).contains("resources/list"), "{res}");
    assert_error_envelope(&res);
}
