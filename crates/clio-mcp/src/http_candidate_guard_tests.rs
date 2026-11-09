// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate `2026-07-28` guard scenarios
//! driven through the real HTTP adapter: the conditional headers, the GET
//! refusal, the absent session identifier, the removed methods, and the
//! negative self-check.
//!
//! ## Owns
//! - The conditional-header, verb, session, and removed-method scenarios
//! - The negative self-check that a successful response to a removed method
//!   fails the candidate check
//!
//! ## Does not own
//! - The candidate HTTP seam (`http_candidate`), its guards
//!   (`method_guard`, `conditional_http`), or its admission (`stateless`)
//! - The shared server, request, and checker fixtures
//!   (`http_candidate_fixtures`)
//! - The scenario registry and the metadata and envelope scenarios (see
//!   `http_candidate_tests`)
//!
//! ## Boundary
//!
//! Changes to how the candidate composes its guards over HTTP should normally
//! require changes here. Changes to the active transport or the active pin
//! should not.
//!
//! Keep this module focused on the candidate's HTTP guard scenarios.

use crate::http::http_test_support_tests::request as raw_request;
use crate::http_candidate_fixtures::{
    CANDIDATE_FIXTURE_REVISION, body, check_error_envelope, check_removed_method_refusal, headers,
    name_headers, post, spawn_candidate,
};
use crate::stateless_fixtures::{request, required_meta};
use serde_json::json;

#[test]
fn http_conditional_method_header_mismatch_refused() {
    let port = spawn_candidate();
    let frame = request(
        "server/discover",
        &required_meta(CANDIDATE_FIXTURE_REVISION),
    );
    let mismatched = post(port, &frame, &headers("tools/list"));
    assert_eq!(mismatched.status, 400);
    let answer = body(&mismatched);
    check_error_envelope(&answer, 7).expect("a mismatched method header is refused");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("Mcp-Method"),
        "{answer}"
    );

    let absent = post(
        port,
        &frame,
        &format!("Mcp-Protocol-Version: {CANDIDATE_FIXTURE_REVISION}\r\n"),
    );
    assert_eq!(absent.status, 400);
    let answer = body(&absent);
    check_error_envelope(&answer, 7).expect("an absent method header is refused");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("Mcp-Method"),
        "{answer}"
    );
}

#[test]
fn http_conditional_name_header_mismatch_refused() {
    let port = spawn_candidate();
    let frame = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {
            "_meta": required_meta(CANDIDATE_FIXTURE_REVISION),
            "name": "consolidate",
            "arguments": { "bank": "fixture-bank" }
        }
    });
    let mismatched = post(port, &frame, &name_headers("tools/call", "elsewhere"));
    assert_eq!(mismatched.status, 400);
    let answer = body(&mismatched);
    check_error_envelope(&answer, 7).expect("a mismatched name header is refused");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("Mcp-Name"),
        "{answer}"
    );

    let absent = post(port, &frame, &headers("tools/call"));
    assert_eq!(absent.status, 400);
    let answer = body(&absent);
    check_error_envelope(&answer, 7).expect("an absent name header is refused");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("Mcp-Name"),
        "{answer}"
    );
}

#[test]
fn http_get_is_refused() {
    let port = spawn_candidate();
    let response = raw_request(port, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(response.status, 405, "{:?}", response.headers);
    assert_eq!(
        response.headers.get("allow").map(String::as_str),
        Some("POST"),
        "the refusal names the one accepted verb"
    );
    assert!(response.body.is_empty(), "{:?}", response.body);
}

#[test]
fn http_no_session_identifier_minted_or_required() {
    let port = spawn_candidate();
    let discover = post(
        port,
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
        &headers("server/discover"),
    );
    assert_eq!(discover.status, 200);
    assert!(
        !discover.headers.contains_key("mcp-session-id"),
        "the candidate mints no session identifier"
    );

    let mut extra = headers("tools/list");
    extra.push_str("MCP-Session-Id: stale-and-ignored\r\n");
    let list = post(
        port,
        &request("tools/list", &required_meta(CANDIDATE_FIXTURE_REVISION)),
        &extra,
    );
    assert_eq!(
        list.status, 200,
        "a session identifier is neither required nor checked"
    );
    assert!(
        !list.headers.contains_key("mcp-session-id"),
        "the candidate mints no session identifier"
    );
}

#[test]
fn http_removed_methods_are_refused() {
    let port = spawn_candidate();
    for method in ["initialize", "ping", "logging/setLevel"] {
        let response = post(
            port,
            &request(method, &required_meta(CANDIDATE_FIXTURE_REVISION)),
            &headers(method),
        );
        assert_eq!(response.status, 200, "{method}: {}", response.body);
        let answer = body(&response);
        check_removed_method_refusal(&answer, method)
            .unwrap_or_else(|error| panic!("{method}: {error}"));
    }
}

#[test]
fn a_successful_response_to_a_removed_method_fails_the_candidate_check() {
    let synthetic_success = json!({
        "jsonrpc": "2.0", "id": 7,
        "result": { "resultType": "complete" }
    });
    assert!(
        check_removed_method_refusal(&synthetic_success, "initialize").is_err(),
        "a successful response to a removed method must fail the candidate check"
    );

    let refused = json!({
        "jsonrpc": "2.0", "id": 7,
        "error": { "code": -32601, "message": "method `ping` not found" }
    });
    check_removed_method_refusal(&refused, "ping")
        .expect("the checker accepts an actual refusal, so it is not always failing");
}
