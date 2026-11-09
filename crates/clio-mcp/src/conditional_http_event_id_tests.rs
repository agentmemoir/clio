// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `Last-Event-ID` fixtures of the candidate
//! conditional-header guard: no method this revision serves resumes a stream,
//! so the guard reads no event id and a request carrying one is decided by
//! `Mcp-Method` and `Mcp-Name` alone.
//!
//! ## Owns
//! - Every assertion about a request that carries a `Last-Event-ID` header
//!
//! ## Does not own
//! - The guard itself (`conditional_http`)
//! - The other header fixtures of that guard (`conditional_http_tests`)
//!
//! ## Boundary
//!
//! Changes to what the guard reads should normally require changes here.
//! Changes to the decoded name's rules should not.
//!
//! Keep this module focused on the event-id header.

use super::{ERR_HEADER_MISMATCH, MCP_METHOD_HEADER, MCP_NAME_HEADER, refuse};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The header naming the last event a client claims to have seen; the guard
/// never reads it.
const LAST_EVENT_ID_HEADER: &str = "last-event-id";

/// The one value the tests below send in that header.
const LAST_EVENT_ID: (&str, &str) = (LAST_EVENT_ID_HEADER, "7");

/// Standard `Base64` of `clio_read`, in the sentinel form a client sends.
const ENCODED_READ: &str = "=?base64?Y2xpb19yZWFk?=";

/// A `tools/call` body naming one tool.
fn tool_call(name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": {} }
    })
}

/// The candidate refusal one request draws, with its body parsed.
///
/// `None` is a request the guard accepts.
fn refusal(headers: &[(&str, &str)], body: &Value) -> Option<(u16, Value)> {
    let supplied: HashMap<&str, &str> = headers.iter().copied().collect();
    let (status, text) = refuse(|name: &str| supplied.get(name).copied(), body)?;
    Some((
        status,
        serde_json::from_str(&text).expect("the refusal body is JSON"),
    ))
}

#[test]
fn a_tool_call_with_an_encoded_name_and_a_last_event_id_is_not_refused() {
    let headers = [
        (MCP_METHOD_HEADER, "tools/call"),
        (MCP_NAME_HEADER, ENCODED_READ),
        LAST_EVENT_ID,
    ];
    assert!(
        refusal(&headers, &tool_call("clio_read")).is_none(),
        "the guard reads no event id: {headers:?}"
    );
}

#[test]
fn a_listing_with_a_last_event_id_and_no_name_header_is_not_refused() {
    let body = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    let headers = [(MCP_METHOD_HEADER, "tools/list"), LAST_EVENT_ID];
    assert!(
        refusal(&headers, &body).is_none(),
        "a listing names no resource to compare"
    );
}

#[test]
fn a_last_event_id_does_not_excuse_a_missing_method_header() {
    let headers = [(MCP_NAME_HEADER, "clio_read"), LAST_EVENT_ID];
    let body = tool_call("clio_read");
    let (status, envelope) = refusal(&headers, &body).expect("the guard refuses this request");
    assert_eq!(status, 400, "{envelope}");
    assert_eq!(
        envelope["error"]["code"].as_i64(),
        Some(ERR_HEADER_MISMATCH),
        "the header-mismatch code: {envelope}"
    );
    let message = envelope["error"]["message"]
        .as_str()
        .expect("the refusal names a reason")
        .to_lowercase();
    assert!(
        message.contains(MCP_METHOD_HEADER),
        "the refusal names the missing header: {message}"
    );
    assert!(
        !message.contains(LAST_EVENT_ID_HEADER),
        "the refusal is not about the event id: {message}"
    );
}

#[test]
fn an_unknown_method_with_agreeing_headers_and_a_last_event_id_is_not_refused() {
    let body = json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/read", "params": {} });
    let headers = [(MCP_METHOD_HEADER, "resources/read"), LAST_EVENT_ID];
    assert!(
        refusal(&headers, &body).is_none(),
        "an unknown method stays with the JSON-RPC layer"
    );
}
