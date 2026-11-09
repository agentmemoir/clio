// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate `2026-07-28` conformance
//! scenarios driven through the real stdio adapter: required and optional
//! request metadata, discovery identity, and the success and error envelopes.
//!
//! ## Owns
//! - The scenario registry: each candidate wire scenario's id, implementation
//!   owner, and bounded check receipt
//! - The `serve_candidate` scenarios and the negative self-checks
//!
//! ## Does not own
//! - The candidate admission and dispatch rules (`stateless`)
//! - The active stdio adapter and its framing (`stdio`)
//! - The active revision pin (see the crate's conformance suite)
//!
//! ## Boundary
//!
//! Changes to a candidate wire scenario should normally require changes here.
//! Changes to the active transport or the active pin should not.
//!
//! Keep this module focused on the candidate's stdio conformance scenarios.

use super::serve_candidate;
use crate::conditional_http::MCP_METHOD_HEADER;
use crate::runtime::McpState;
use crate::stateless::{CANDIDATE_REVISION, PROTOCOL_VERSION_HEADER, admit};
use crate::stateless_fixtures::{full_meta, request, required_meta, state};
use serde_json::{Value, json};
use std::io::Cursor;

/// The candidate fixture pin: the revision a future activation test expects.
///
/// This is a test expectation, not a live protocol setting. The active pin
/// stays [`crate::MCP_PROTOCOL_REVISION`].
const CANDIDATE_FIXTURE_REVISION: &str = "2026-07-28";

/// One candidate wire scenario and the bounded check that proves it.
struct Scenario {
    /// Stable scenario id.
    id: &'static str,
    /// The candidate implementation the scenario covers.
    owner: &'static str,
    /// The one command that runs the test proving the scenario.
    receipt: &'static str,
}

/// Every candidate stdio wire scenario this module covers.
const SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "stdio/required-metadata",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib stdio_required_metadata_refusal",
    },
    Scenario {
        id: "stdio/optional-metadata",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib stdio_optional_metadata_accepted",
    },
    Scenario {
        id: "stdio/discovery-identity",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib stdio_discovery_identity_lives_in_result_meta",
    },
    Scenario {
        id: "stdio/success-envelope",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib stdio_success_envelope_shape",
    },
    Scenario {
        id: "stdio/error-envelope",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib stdio_error_envelope_shape",
    },
    Scenario {
        id: "stdio/removed-request",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib stdio_removed_request_is_refused_not_served",
    },
    Scenario {
        id: "harness/wrong-header",
        owner: "conformance harness",
        receipt: "cargo test -p clio-mcp --lib a_deliberately_wrong_header_value_fails_the_checker",
    },
    Scenario {
        id: "harness/wrong-result-shape",
        owner: "conformance harness",
        receipt: "cargo test -p clio-mcp --lib a_deliberately_wrong_result_shape_fails_the_checkers",
    },
];

/// Run the candidate stdio adapter over the frames and parse every answer line.
fn answers(state: &McpState, frames: &[&Value]) -> Vec<Value> {
    let mut input = String::new();
    for frame in frames {
        input.push_str(&serde_json::to_string(*frame).expect("a fixture serializes"));
        input.push('\n');
    }
    let mut output = Vec::new();
    serve_candidate(state, Cursor::new(input.into_bytes()), &mut output)
        .expect("the candidate stdio adapter serves the frames");
    let text = String::from_utf8(output).expect("the adapter writes UTF-8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("one compact JSON answer per line"))
        .collect()
}

/// Run one frame and return its one answer.
fn answer_one(state: &McpState, frame: &Value) -> Value {
    let mut answers = answers(state, &[frame]);
    assert_eq!(answers.len(), 1, "one request answers one line: {frame}");
    answers.remove(0)
}

/// A success envelope: JSON-RPC 2.0, the request id, one result, and no error.
fn check_success_envelope(envelope: &Value, id: i64) -> Result<(), String> {
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
fn check_error_envelope(envelope: &Value, id: i64) -> Result<(), String> {
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
fn check_complete_result(result: &Value) -> Result<(), String> {
    if result["resultType"] != json!("complete") {
        return Err(format!("the result is not complete: {result}"));
    }
    Ok(())
}

/// Discovery identity lives in `result._meta` and never at the top of the
/// result.
fn check_discovery_identity(result: &Value) -> Result<(), String> {
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

#[test]
fn every_scenario_records_one_owner_and_one_bounded_check() {
    let mut ids: Vec<&str> = Vec::new();
    for scenario in SCENARIOS {
        let filter = scenario
            .receipt
            .strip_prefix("cargo test -p clio-mcp --lib ")
            .unwrap_or_else(|| panic!("{}: the receipt runs the crate's lib tests", scenario.id));
        assert!(
            !scenario.id.is_empty()
                && !scenario.owner.is_empty()
                && !filter.is_empty()
                && !filter.contains(' '),
            "{}: an id, an owner, and one bounded receipt are required",
            scenario.id
        );
        assert!(
            !ids.contains(&scenario.id),
            "{}: the scenario id is unique",
            scenario.id
        );
        ids.push(scenario.id);
    }
    assert_eq!(ids.len(), 8, "every covered scenario is registered");
}

#[test]
fn the_candidate_fixture_pin_is_a_future_expectation() {
    assert_eq!(
        CANDIDATE_FIXTURE_REVISION, CANDIDATE_REVISION,
        "the fixture pin follows the candidate dispatcher"
    );
    assert_eq!(
        crate::MCP_PROTOCOL_REVISION,
        "2025-11-25",
        "the live pin does not move"
    );
    assert_ne!(
        CANDIDATE_FIXTURE_REVISION,
        crate::MCP_PROTOCOL_REVISION,
        "the candidate pin is not a second live protocol setting"
    );
}

#[test]
fn stdio_required_metadata_refusal() {
    let state = state();
    let cases = [
        (
            "absent _meta",
            json!({ "jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": {} }),
            -32020,
            "MCP-Protocol-Version",
        ),
        (
            "absent protocolVersion",
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "server/discover",
                "params": { "_meta": { "io.modelcontextprotocol/clientCapabilities": {} } }
            }),
            -32020,
            "MCP-Protocol-Version",
        ),
        (
            "absent clientCapabilities",
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "server/discover",
                "params": {
                    "_meta": { "io.modelcontextprotocol/protocolVersion": CANDIDATE_FIXTURE_REVISION }
                }
            }),
            -32602,
            "clientCapabilities",
        ),
    ];
    for (label, frame, code, names) in cases {
        let answer = answer_one(&state, &frame);
        check_error_envelope(&answer, 7).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(answer["error"]["code"], json!(code), "{label}: {answer}");
        let message = answer["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{label}: an error message"));
        assert!(message.contains(names), "{label}: {answer}");
    }
}

#[test]
fn stdio_optional_metadata_accepted() {
    let state = state();
    let required = answer_one(
        &state,
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
    );
    let full = answer_one(
        &state,
        &request("server/discover", &full_meta(CANDIDATE_FIXTURE_REVISION)),
    );
    check_success_envelope(&required, 7).expect("the required-only request is served");
    check_success_envelope(&full, 7).expect("the full-metadata request is served");
    assert_eq!(
        required["result"], full["result"],
        "optional metadata never changes the result"
    );
}

#[test]
fn stdio_discovery_identity_lives_in_result_meta() {
    let answer = answer_one(
        &state(),
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
    );
    check_success_envelope(&answer, 7).expect("discovery is served");
    let result = &answer["result"];
    check_complete_result(result).expect("discovery is a complete result");
    check_discovery_identity(result).expect("discovery carries its identity in result metadata");
}

#[test]
fn stdio_success_envelope_shape() {
    let answer = answer_one(
        &state(),
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
    );
    check_success_envelope(&answer, 7).expect("a served request is a success envelope");
    assert_eq!(answer["result"]["resultType"], json!("complete"));
    assert!(answer.get("error").is_none(), "{answer}");
}

#[test]
fn stdio_error_envelope_shape() {
    let frames = [
        request("resources/list", &required_meta(CANDIDATE_FIXTURE_REVISION)),
        request("no/such-method", &required_meta(CANDIDATE_FIXTURE_REVISION)),
    ];
    let answers = answers(&state(), &[&frames[0], &frames[1]]);
    assert_eq!(answers.len(), 2, "each request answers one line");
    for answer in &answers {
        check_error_envelope(answer, 7).expect("an unserved request is an error envelope");
        assert_eq!(answer["error"]["code"], json!(-32601), "{answer}");
    }
}

#[test]
fn stdio_removed_request_is_refused_not_served() {
    let frame = request("initialize", &required_meta(CANDIDATE_FIXTURE_REVISION));
    let answer = answer_one(&state(), &frame);
    check_error_envelope(&answer, 7).expect("a removed request is refused");
    assert_eq!(
        answer["error"]["code"],
        json!(-32601),
        "a removed method is not found: {answer}"
    );
    assert!(
        answer.get("result").is_none(),
        "a removed method is never served: {answer}"
    );
}

#[test]
fn a_deliberately_wrong_header_value_fails_the_checker() {
    let state = state();
    let frame = serde_json::to_string(&request(
        "server/discover",
        &required_meta(CANDIDATE_FIXTURE_REVISION),
    ))
    .expect("a fixture serializes");
    let wrong_version = admit(
        &state,
        |header| match header {
            MCP_METHOD_HEADER => Some("server/discover"),
            PROTOCOL_VERSION_HEADER => Some("1900-01-01"),
            _ => None,
        },
        &frame,
    );
    assert!(
        wrong_version.is_err(),
        "a wrong version header is refused, not served: {wrong_version:?}"
    );
    let wrong_method = admit(
        &state,
        |header| match header {
            MCP_METHOD_HEADER => Some("tools/list"),
            PROTOCOL_VERSION_HEADER => Some(CANDIDATE_FIXTURE_REVISION),
            _ => None,
        },
        &frame,
    );
    assert!(
        wrong_method.is_err(),
        "a wrong method header is refused, not served: {wrong_method:?}"
    );
}

#[test]
fn a_deliberately_wrong_result_shape_fails_the_checkers() {
    let partial = json!({ "resultType": "partial" });
    assert!(
        check_complete_result(&partial).is_err(),
        "a result that is not complete fails the check"
    );

    let missing_identity = json!({ "resultType": "complete", "_meta": {} });
    assert!(
        check_discovery_identity(&missing_identity).is_err(),
        "a result without identity fails the check"
    );

    let top_level_identity = json!({
        "resultType": "complete",
        "serverInfo": { "name": "wrong", "version": "0" }
    });
    assert!(
        check_discovery_identity(&top_level_identity).is_err(),
        "a top-level identity fails the check"
    );

    let ambiguous = json!({
        "jsonrpc": "2.0", "id": 7,
        "result": {},
        "error": { "code": -32601, "message": "refused" }
    });
    assert!(
        check_success_envelope(&ambiguous, 7).is_err(),
        "an envelope carrying an error is no success"
    );
    assert!(
        check_error_envelope(&ambiguous, 7).is_err(),
        "an envelope carrying a result is no error"
    );
}
