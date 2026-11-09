// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures of the candidate
//! `2026-07-28` HTTP conformance tests: a live candidate server on an
//! OS-assigned port, request helpers, and the result checkers both test
//! modules assert with.
//!
//! ## Owns
//! - The candidate fixture revision pin and the live-server helper
//! - The candidate conditional-header builders and the POST helper
//! - The success, error, discovery, and removed-method checkers
//!
//! ## Does not own
//! - Any scenario or assertion (see `http_candidate_tests` and
//!   `http_candidate_guard_tests`)
//! - The candidate seam (`http_candidate`) or its rules (`stateless`,
//!   `conditional_http`, `method_guard`)
//!
//! ## Boundary
//!
//! Changes to how the candidate HTTP tests build or check a response should
//! normally require changes here. Changes to what a scenario asserts should
//! not.
//!
//! Keep this module free of test functions.

use crate::http::http_test_support_tests::{Response, post_json};
use crate::http_candidate::serve_candidate;
use crate::stateless_fixtures::state;
use serde_json::{Value, json};
use std::net::TcpListener;

/// The candidate fixture pin: the revision a future activation test expects.
///
/// This is a test expectation, not a live protocol setting. The active pin
/// stays [`crate::MCP_PROTOCOL_REVISION`].
pub(crate) const CANDIDATE_FIXTURE_REVISION: &str = "2026-07-28";

/// Serve the candidate endpoint on an OS-assigned port and return that port.
pub(crate) fn spawn_candidate() -> u16 {
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        serve_candidate(&state, &listener);
    });
    port
}

/// The candidate conditional headers for one method, ending the header block.
pub(crate) fn headers(method: &str) -> String {
    format!("Mcp-Protocol-Version: {CANDIDATE_FIXTURE_REVISION}\r\nMcp-Method: {method}\r\n")
}

/// The candidate headers for one tool call, carrying its resource name.
pub(crate) fn name_headers(method: &str, name: &str) -> String {
    format!(
        "Mcp-Protocol-Version: {CANDIDATE_FIXTURE_REVISION}\r\nMcp-Method: {method}\r\nMcp-Name: {name}\r\n"
    )
}

/// POST one candidate frame and return the response.
pub(crate) fn post(port: u16, frame: &Value, extra: &str) -> Response {
    let body = serde_json::to_string(frame).expect("a fixture serializes");
    post_json(port, &body, extra)
}

/// Parse a response body as JSON.
pub(crate) fn body(response: &Response) -> Value {
    serde_json::from_str(&response.body).expect("the response body is JSON")
}

/// A success envelope: JSON-RPC 2.0, the request id, one result, and no error.
pub(crate) fn check_success_envelope(envelope: &Value, id: i64) -> Result<(), String> {
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("not JSON-RPC 2.0: {envelope}"));
    }
    if envelope.get("result").is_none() {
        return Err(format!("a success envelope carries a result: {envelope}"));
    }
    if envelope.get("error").is_some() {
        return Err(format!("a success envelope carries no error: {envelope}"));
    }
    if envelope["id"] != json!(id) {
        return Err(format!("the envelope id is not `{id}`: {envelope}"));
    }
    Ok(())
}

/// An error envelope: JSON-RPC 2.0, the request id, one error, no result, and
/// no result type anywhere.
pub(crate) fn check_error_envelope(envelope: &Value, id: i64) -> Result<(), String> {
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("not JSON-RPC 2.0: {envelope}"));
    }
    if envelope.get("error").is_none() {
        return Err(format!("an error envelope carries an error: {envelope}"));
    }
    if envelope.get("result").is_some() {
        return Err(format!("an error envelope carries no result: {envelope}"));
    }
    if envelope["id"] != json!(id) {
        return Err(format!("the envelope id is not `{id}`: {envelope}"));
    }
    let raw = serde_json::to_string(envelope).expect("an envelope serializes");
    if raw.contains("resultType") {
        return Err(format!(
            "an error envelope claims no result type: {envelope}"
        ));
    }
    Ok(())
}

/// A complete result: the revision's one completeness marker.
pub(crate) fn check_complete_result(result: &Value) -> Result<(), String> {
    if result["resultType"] != json!("complete") {
        return Err(format!("the result is not complete: {result}"));
    }
    Ok(())
}

/// Discovery identity lives in `result._meta` and never at the top of the
/// result.
pub(crate) fn check_discovery_identity(result: &Value) -> Result<(), String> {
    if result.get("serverInfo").is_some() {
        return Err(format!(
            "identity is never a top-level result field: {result}"
        ));
    }
    let info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    if info["name"] != json!(env!("CARGO_PKG_NAME")) {
        return Err(format!("the identity name is the crate name: {result}"));
    }
    if info["version"] != json!(env!("CARGO_PKG_VERSION")) {
        return Err(format!(
            "the identity version is the crate version: {result}"
        ));
    }
    Ok(())
}

/// A removed method's answer: an error envelope with the not-found code and a
/// message naming the method, and no result at all.
pub(crate) fn check_removed_method_refusal(envelope: &Value, method: &str) -> Result<(), String> {
    if envelope.get("result").is_some() {
        return Err(format!("`{method}` was served: {envelope}"));
    }
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("`{method}`: not JSON-RPC 2.0: {envelope}"));
    }
    if envelope["error"]["code"] != json!(-32601) {
        return Err(format!("`{method}`: not the not-found code: {envelope}"));
    }
    let message = envelope["error"]["message"]
        .as_str()
        .ok_or_else(|| format!("`{method}`: no error message: {envelope}"))?;
    if !message.contains(method) {
        return Err(format!(
            "`{method}`: the message does not name it: {envelope}"
        ));
    }
    Ok(())
}
