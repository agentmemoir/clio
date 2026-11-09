// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate
//! `2026-07-28` admission path: the conditional headers, JSON syntax, the
//! required version header, the required metadata, header/body equality, and
//! the supported revision, in that order.
//!
//! ## Owns
//! - Every assertion about `admit` and its refusal mapping
//! - The claim that a refused request never reaches method dispatch
//! - The claim that the served pin and the live route stay off this surface
//!
//! ## Does not own
//! - The dispatcher and the admission path themselves (`stateless`)
//! - The conditional-header guard (`conditional_http`)
//! - The request shapes these fixtures send (see `stateless_fixtures`)
//!
//! ## Boundary
//!
//! Changes to the candidate admission order or its refusal codes should
//! normally require changes here. Changes to the pinned revision's suites
//! should not.
//!
//! Keep this module focused on the admission path's fixtures.

use super::{CANDIDATE_REVISION, PROTOCOL_VERSION_HEADER, SUPPORTED_REVISIONS, admit};
use crate::conditional_http::MCP_METHOD_HEADER;
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use crate::stateless_fixtures::{code, message, request, required_meta, state};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The admission outcome: the served answer, or the refusal status and body.
type Outcome = Result<Option<Value>, (u16, String)>;

/// Run the admission path over one frame with the given headers.
fn run(state: &McpState, headers: &[(&str, &str)], frame: &str) -> Outcome {
    let supplied: HashMap<&str, &str> = headers.iter().copied().collect();
    admit(state, |name: &str| supplied.get(name).copied(), frame)
}

/// The refusal of one frame, with its body parsed.
fn refused(state: &McpState, headers: &[(&str, &str)], frame: &str) -> (u16, Value) {
    let (status, body) = run(state, headers, frame).expect_err("the admission refuses the frame");
    (
        status,
        serde_json::from_str(&body).expect("the refusal body is JSON"),
    )
}

/// The two headers a candidate request carries, with the version it declares.
fn headers(version: &str) -> Vec<(&str, &str)> {
    vec![
        (MCP_METHOD_HEADER, "tools/list"),
        (PROTOCOL_VERSION_HEADER, version),
    ]
}

/// Serialize one fixture message into the frame the admission reads.
fn frame(msg: &Value) -> String {
    serde_json::to_string(msg).expect("a fixture serializes")
}

/// A `tools/list` frame whose metadata declares `version`.
fn list_frame(version: &str) -> String {
    frame(&request("tools/list", &required_meta(version)))
}

#[test]
fn a_matching_supported_version_reaches_dispatch() {
    let state = state();
    let msg = request("tools/list", &required_meta(CANDIDATE_REVISION));
    let served = run(&state, &headers(CANDIDATE_REVISION), &frame(&msg))
        .expect("the request is admitted")
        .expect("a request carrying an id is answered");
    let expected = super::dispatch(&state, &msg).expect("dispatch answers");
    assert_eq!(served, expected, "the admission serves the dispatch result");
    assert!(served.get("error").is_none(), "{served}");
}

#[test]
fn a_matching_unsupported_version_is_refused() {
    let (status, body) = refused(&state(), &headers("2025-11-25"), &list_frame("2025-11-25"));
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), -32022, "{body}");
    assert_eq!(
        body["error"]["data"]["supported"],
        json!(["2026-07-28"]),
        "{body}"
    );
    assert_eq!(
        body["error"]["data"]["requested"],
        json!("2025-11-25"),
        "{body}"
    );
    assert!(body.get("result").is_none(), "{body}");
}

#[test]
fn an_unequal_header_and_body_version_is_refused() {
    let (status, body) = refused(
        &state(),
        &headers(CANDIDATE_REVISION),
        &list_frame("2025-11-25"),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), -32020, "{body}");
    assert!(
        message(&body).contains("MCP-Protocol-Version"),
        "names the header: {body}"
    );
    assert!(body.get("result").is_none(), "{body}");
}

#[test]
fn a_missing_or_empty_version_header_is_refused() {
    let state = state();
    let frame = list_frame(CANDIDATE_REVISION);
    for (label, supplied) in [
        ("absent", vec![(MCP_METHOD_HEADER, "tools/list")]),
        ("empty", headers("")),
    ] {
        let (status, body) = refused(&state, &supplied, &frame);
        assert_eq!(status, 400, "{label}: {body}");
        assert_eq!(code(&body), -32020, "{label}: {body}");
        assert!(
            message(&body).contains("MCP-Protocol-Version"),
            "{label}: names the header: {body}"
        );
    }
}

#[test]
fn missing_required_metadata_is_invalid_params_at_http_400() {
    let frame = frame(&json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/list",
        "params": { "_meta": { "io.modelcontextprotocol/clientCapabilities": {} } }
    }));
    let (status, body) = refused(&state(), &headers(CANDIDATE_REVISION), &frame);
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), -32602, "{body}");
    assert!(
        message(&body).contains("protocolVersion"),
        "names the key: {body}"
    );
    assert!(body.get("result").is_none(), "{body}");
}

#[test]
fn both_optional_metadata_fields_may_be_absent() {
    let meta = required_meta(CANDIDATE_REVISION);
    assert!(meta.get("io.modelcontextprotocol/clientInfo").is_none());
    assert!(meta.get("io.modelcontextprotocol/logLevel").is_none());
    let served = run(
        &state(),
        &headers(CANDIDATE_REVISION),
        &list_frame(CANDIDATE_REVISION),
    )
    .expect("the request is admitted")
    .expect("a request carrying an id is answered");
    assert!(served.get("error").is_none(), "{served}");
}

#[test]
fn invalid_json_is_a_parse_error_before_the_header_check() {
    let (status, body) = refused(&state(), &[], "{ not json");
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), -32700, "{body}");
}

#[test]
fn a_refused_request_never_reaches_dispatch() {
    let state = state();
    // This frame alone dispatches; the unequal header refuses it first.
    let frame = list_frame(CANDIDATE_REVISION);
    let (status, body) = refused(&state, &headers("2025-11-25"), &frame);
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), -32020, "{body}");
    assert!(body.get("result").is_none(), "{body}");
}

#[test]
fn the_served_pin_and_live_route_stay_off_the_candidate_surface() {
    assert_eq!(crate::MCP_PROTOCOL_REVISION, "2025-11-25");
    assert_eq!(SUPPORTED_REVISIONS.len(), 1);
    assert_eq!(SUPPORTED_REVISIONS[0], CANDIDATE_REVISION);
    assert_ne!(CANDIDATE_REVISION, crate::MCP_PROTOCOL_REVISION);
    assert_eq!(McpHandler::new(state()).protocol_revision(), "2025-11-25");
    let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("the crate root is readable");
    assert!(
        lib.contains("#[cfg(test)]\nmod stateless;"),
        "the candidate module stays test-only"
    );
    assert!(
        lib.contains("#[cfg(test)]\nmod conditional_http;"),
        "the guard module stays test-only"
    );
}
