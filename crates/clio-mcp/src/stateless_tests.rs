// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate
//! `2026-07-28` dispatcher: metadata validation, error codes, the capability
//! gate, discovery, removal of the handshake, and the guard that keeps the pin
//! where it is.
//!
//! ## Owns
//! - Every assertion about `dispatch`, `required_capability`, and the revision
//!   constants
//!
//! ## Does not own
//! - The dispatcher itself (`stateless`)
//! - The request shapes these fixtures send (see `stateless_fixtures`)
//! - Transport conformance or the pinned-revision contract (see the crate's
//!   integration suites)
//!
//! ## Boundary
//!
//! Changes to the candidate dispatcher's behavior should normally require
//! changes here. Changes to the pinned revision's suites should not.
//!
//! Keep this module focused on the candidate dispatcher's fixtures.

use super::{CACHE_HINTS, CANDIDATE_REVISION, SUPPORTED_REVISIONS, dispatch, required_capability};
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use crate::stateless_fixtures::{
    code, elicitation_answer, eliciting_meta, full_meta, message, request, required_meta,
    sampling_answer, server_info, state, tool_call,
};
use serde_json::{Value, json};

fn answer(state: &McpState, msg: &Value) -> Value {
    dispatch(state, msg).expect("a request carrying an id is answered")
}

fn assert_states_agree(first: &McpState, second: &McpState, msg: &Value) -> Value {
    let (one, two) = (answer(first, msg), answer(second, msg));
    assert!(one.get("error").is_none(), "{msg}: {one}");
    assert!(two.get("error").is_none(), "{msg}: {two}");
    assert_eq!(one, two, "{msg}");
    one
}

fn assert_invalid_params(meta: Option<Value>, key: &str) {
    let state = state();
    let msg = meta.map_or_else(
        || json!({ "jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": {} }),
        |meta| request("server/discover", &meta),
    );
    let res = answer(&state, &msg);
    assert_eq!(code(&res), -32602, "case `{key}`: {res}");
    assert!(message(&res).contains(key), "names `{key}`: {res}");
}

#[test]
fn discover_reports_the_candidate_revision_and_its_capabilities() {
    let res = answer(
        &state(),
        &request("server/discover", &required_meta(CANDIDATE_REVISION)),
    );
    let result = &res["result"];
    assert_eq!(res["jsonrpc"], json!("2.0"));
    assert_eq!(res["id"], json!(7));
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["supportedVersions"], json!([CANDIDATE_REVISION]));
    assert_eq!(
        result["capabilities"]["tools"],
        json!({ "listChanged": false })
    );
    assert_eq!(result["ttlMs"], json!(CACHE_HINTS.ttl_ms));
    assert_eq!(result["cacheScope"], json!(CACHE_HINTS.scope));
}

#[test]
fn identity_is_never_a_top_level_result_field() {
    let state = state();
    for method in ["server/discover", "tools/list"] {
        let res = answer(&state, &request(method, &required_meta(CANDIDATE_REVISION)));
        let result = &res["result"];
        assert!(result.get("serverInfo").is_none(), "{method}: {res}");
        let info = server_info(result);
        assert_eq!(info["name"], json!(env!("CARGO_PKG_NAME")), "{method}");
        assert_eq!(
            info["version"],
            json!(env!("CARGO_PKG_VERSION")),
            "{method}"
        );
    }
}

#[test]
fn absent_meta_is_invalid_params() {
    assert_invalid_params(None, "_meta");
}

#[test]
fn absent_protocol_version_is_invalid_params() {
    assert_invalid_params(
        Some(json!({ "io.modelcontextprotocol/clientCapabilities": {} })),
        "protocolVersion",
    );
}

#[test]
fn non_string_protocol_version_is_invalid_params() {
    assert_invalid_params(
        Some(json!({
            "io.modelcontextprotocol/protocolVersion": ["2026-07-28"],
            "io.modelcontextprotocol/clientCapabilities": {}
        })),
        "protocolVersion",
    );
}

#[test]
fn absent_client_capabilities_is_invalid_params() {
    assert_invalid_params(
        Some(json!({ "io.modelcontextprotocol/protocolVersion": CANDIDATE_REVISION })),
        "clientCapabilities",
    );
}

#[test]
fn non_object_client_capabilities_is_invalid_params() {
    assert_invalid_params(
        Some(json!({
            "io.modelcontextprotocol/protocolVersion": CANDIDATE_REVISION,
            "io.modelcontextprotocol/clientCapabilities": []
        })),
        "clientCapabilities",
    );
}

#[test]
fn optional_metadata_never_changes_the_discovery_result() {
    let state = state();
    for (label, meta) in [
        ("omitted", required_meta(CANDIDATE_REVISION)),
        ("present", full_meta(CANDIDATE_REVISION)),
    ] {
        let res = answer(&state, &request("server/discover", &meta));
        assert!(
            res.get("error").is_none(),
            "{label}: optional keys never reject: {res}"
        );
        let result = &res["result"];
        assert_eq!(result["resultType"], json!("complete"), "{label}: {res}");
        assert_eq!(
            result["supportedVersions"],
            json!([CANDIDATE_REVISION]),
            "{label}: {res}"
        );
        assert_eq!(
            server_info(result)["name"],
            json!(env!("CARGO_PKG_NAME")),
            "{label}: {res}"
        );
    }
}

#[test]
fn a_revision_outside_the_supported_set_is_refused() {
    let state = state();
    for requested in ["2025-11-25", "1900-01-01"] {
        let res = answer(
            &state,
            &request("server/discover", &required_meta(requested)),
        );
        assert_eq!(code(&res), -32022, "{requested}: {res}");
        let data = &res["error"]["data"];
        assert_eq!(data["supported"], json!(["2026-07-28"]), "{requested}");
        assert_eq!(data["requested"], json!(requested), "{requested}");
    }
}

#[test]
fn only_a_tool_call_that_answers_input_requests_needs_a_capability() {
    let cases = [
        ("tools/call", None, None),
        ("tools/call", Some(json!({})), None),
        ("tools/call", Some(json!({ "inputResponses": {} })), None),
        (
            "tools/call",
            Some(elicitation_answer()),
            Some("elicitation"),
        ),
        // A sampling answer carries no `action`, so it demands no capability.
        ("tools/call", Some(sampling_answer()), None),
        // The revision types the answers as an object map; an array is not one.
        (
            "tools/call",
            Some(json!({ "inputResponses": [{ "action": "accept" }] })),
            None,
        ),
        ("tools/list", Some(elicitation_answer()), None),
        ("server/discover", Some(elicitation_answer()), None),
    ];
    for (method, params, want) in cases {
        assert_eq!(
            required_capability(method, params.as_ref()),
            want,
            "{method} with {params:?}"
        );
    }
}

#[test]
fn answering_input_requests_without_elicitation_is_refused() {
    let msg = tool_call(&required_meta(CANDIDATE_REVISION), elicitation_answer());
    let res = answer(&state(), &msg);
    assert_eq!(code(&res), -32021, "{res}");
    assert!(message(&res).contains("elicitation"), "{res}");
    assert_eq!(
        res["error"]["data"]["requiredCapabilities"]["elicitation"],
        json!({}),
        "{res}"
    );
}

#[test]
fn a_tool_call_that_answers_input_is_refused_as_invalid_params() {
    let msg = tool_call(&eliciting_meta(CANDIDATE_REVISION), elicitation_answer());
    let res = answer(&state(), &msg);
    assert_eq!(code(&res), -32602, "{res}");
    assert!(
        message(&res).contains("no pending input request"),
        "states why: {res}"
    );
    assert!(
        res.get("result").is_none(),
        "an answer is refused, never discarded: {res}"
    );
}

#[test]
fn a_sampling_answer_demands_no_capability_and_is_refused_as_invalid_params() {
    let msg = tool_call(&required_meta(CANDIDATE_REVISION), sampling_answer());
    let res = answer(&state(), &msg);
    assert_ne!(code(&res), -32021, "sampling demands no capability: {res}");
    assert_eq!(code(&res), -32602, "{res}");
    assert!(
        message(&res).contains("no pending input request"),
        "states why: {res}"
    );
}

#[test]
fn a_tool_call_carrying_request_state_is_refused_as_invalid_params() {
    let msg = tool_call(
        &eliciting_meta(CANDIDATE_REVISION),
        json!({ "requestState": "opaque-state" }),
    );
    let res = answer(&state(), &msg);
    assert_eq!(code(&res), -32602, "{res}");
    assert!(
        message(&res).contains("no pending input request"),
        "states why: {res}"
    );
}

#[test]
fn ordinary_tool_calls_need_no_client_capability() {
    let res = answer(
        &state(),
        &tool_call(&required_meta(CANDIDATE_REVISION), json!({})),
    );
    assert!(res.get("error").is_none(), "no invented requirement: {res}");
    assert_eq!(res["result"]["isError"], json!(false), "{res}");
}

#[test]
fn a_delegated_tool_call_error_is_passed_through_unstamped() {
    let msg = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {
            "_meta": required_meta(CANDIDATE_REVISION),
            "name": "no-such-tool",
            "arguments": {}
        }
    });
    let res = answer(&state(), &msg);
    assert_eq!(code(&res), -32602, "{res}");
    assert!(message(&res).contains("unknown tool"), "{res}");
    assert!(
        res.get("result").is_none(),
        "the error arm stamps nothing: {res}"
    );
}

#[test]
fn tools_list_serves_the_production_catalogue_for_the_revision() {
    let state = state();
    let res = answer(
        &state,
        &request("tools/list", &required_meta(CANDIDATE_REVISION)),
    );
    let result = &res["result"];
    let visible = McpHandler::tools_list_visible(&state).expect("a known profile");
    assert_eq!(result["tools"], visible["tools"]);
    assert!(
        !result["tools"].as_array().expect("a tool list").is_empty(),
        "the bound catalogue is present"
    );
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["ttlMs"], json!(CACHE_HINTS.ttl_ms));
    assert_eq!(result["cacheScope"], json!(CACHE_HINTS.scope));
    assert_eq!(server_info(result)["name"], json!(env!("CARGO_PKG_NAME")));
}

#[test]
fn initialize_without_metadata_is_a_removed_method() {
    let res = answer(
        &state(),
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "initialize", "params": {} }),
    );
    assert_eq!(code(&res), -32601, "{res}");
    assert!(message(&res).contains("initialize"), "{res}");
}

#[test]
fn logging_set_level_is_a_removed_method() {
    let res = answer(
        &state(),
        &request("logging/setLevel", &required_meta(CANDIDATE_REVISION)),
    );
    assert_eq!(code(&res), -32601, "{res}");
    assert!(message(&res).contains("logging/setLevel"), "{res}");
}

#[test]
fn initialize_and_ping_are_unknown_methods() {
    let state = state();
    for method in ["initialize", "ping"] {
        let res = answer(&state, &request(method, &required_meta(CANDIDATE_REVISION)));
        assert_eq!(code(&res), -32601, "{method}: {res}");
        assert!(message(&res).contains(method), "{method}: {res}");
    }
}

#[test]
fn unknown_method_is_an_unknown_method() {
    let res = answer(
        &state(),
        &request("resources/list", &required_meta(CANDIDATE_REVISION)),
    );
    assert_eq!(code(&res), -32601);
}

#[test]
fn missing_method_is_invalid_request() {
    let res = answer(
        &state(),
        &json!({ "jsonrpc": "2.0", "id": 7, "params": {} }),
    );
    assert_eq!(code(&res), -32600);
    assert_eq!(message(&res), "method is required");
}

#[test]
fn a_message_without_an_id_is_a_notification_and_is_never_answered() {
    let state = state();
    assert!(
        dispatch(
            &state,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none()
    );
    let msg = json!({
        "jsonrpc": "2.0", "method": "tools/list",
        "params": { "_meta": required_meta(CANDIDATE_REVISION) }
    });
    assert!(dispatch(&state, &msg).is_none());
}

#[test]
fn first_message_is_served_without_a_handshake() {
    let res = answer(
        &state(),
        &request("tools/list", &required_meta(CANDIDATE_REVISION)),
    );
    assert!(res.get("error").is_none(), "no prior initialize: {res}");
    assert!(
        !res["result"]["tools"]
            .as_array()
            .expect("a tool list")
            .is_empty(),
        "the catalogue is served: {res}"
    );
}

#[test]
fn dispatch_takes_one_message_and_no_session_handle() {
    let entry: fn(&McpState, &Value) -> Option<Value> = dispatch;
    let res = entry(
        &state(),
        &request("server/discover", &required_meta(CANDIDATE_REVISION)),
    )
    .expect("a request carrying an id is answered");
    assert_eq!(res["result"]["resultType"], json!("complete"), "{res}");
}

#[test]
fn answer_envelopes_carry_no_session_key() {
    let state = state();
    let args = required_meta(CANDIDATE_REVISION);
    let requests = ["server/discover", "tools/list"].map(|method| request(method, &args));
    for msg in requests.into_iter().chain([tool_call(&args, json!({}))]) {
        let res = answer(&state, &msg);
        for object in [&res, &res["result"], &res["result"]["_meta"]] {
            for key in object.as_object().into_iter().flat_map(|map| map.keys()) {
                assert!(!key.to_lowercase().contains("session"), "{key} in {object}");
            }
        }
    }
}

#[test]
fn repeated_calls_in_two_states_answer_identically() {
    let (first, second) = (state(), state());
    let args = required_meta(CANDIDATE_REVISION);
    let list = request("tools/list", &args);
    assert_states_agree(&first, &second, &request("server/discover", &args));
    let before = assert_states_agree(&first, &second, &list);
    let used = answer(&first, &tool_call(&args, json!({})));
    assert!(used.get("error").is_none(), "the prior call: {used}");
    let after = assert_states_agree(&first, &second, &list);
    assert_eq!(after, before, "a prior call leaves the catalogue alone");
}

#[test]
fn the_pin_is_untouched_by_the_candidate() {
    assert_eq!(crate::MCP_PROTOCOL_REVISION, "2025-11-25");
    assert_eq!(
        crate::schema::schema_pack()["mcp_protocol_revision"],
        json!("2025-11-25")
    );
    assert_eq!(SUPPORTED_REVISIONS.len(), 1);
    assert_eq!(SUPPORTED_REVISIONS[0], CANDIDATE_REVISION);
    assert_ne!(CANDIDATE_REVISION, crate::MCP_PROTOCOL_REVISION);
}
