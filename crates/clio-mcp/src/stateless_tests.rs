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
//! - Transport conformance or the pinned-revision contract (see the crate's
//!   integration suites)
//!
//! ## Boundary
//!
//! Changes to the candidate dispatcher's behavior should normally require
//! changes here. Changes to the pinned revision's suites should not.
//!
//! Keep this module focused on the candidate dispatcher's fixtures.

use super::{CANDIDATE_REVISION, SUPPORTED_REVISIONS, dispatch, required_capability};
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Map, Value, json};
use std::sync::Arc;

const BANK: &str = "fixture-bank";

fn state() -> Arc<McpState> {
    let state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    state
        .ops_store
        .register_bank(BANK, "2024-01-01T00:00:00Z")
        .expect("bank fixture");
    state
}

/// Request metadata carrying the two keys the revision requires.
fn required_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Request metadata declaring the client's elicitation capability.
fn eliciting_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } }
    })
}

/// Required metadata plus both optional keys the revision defines.
fn full_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "fixture-client", "version": "0.0.1" },
        "io.modelcontextprotocol/logLevel": "info"
    })
}

fn request(method: &str, meta: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": { "_meta": meta } })
}

fn tool_call(meta: &Value, extra: Value) -> Value {
    let mut params = Map::new();
    params.insert("_meta".into(), meta.clone());
    params.insert("name".into(), json!("consolidate"));
    params.insert("arguments".into(), json!({ "bank": BANK }));
    if let Value::Object(extra) = extra {
        params.extend(extra);
    }
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params })
}

fn answer(state: &McpState, msg: &Value) -> Value {
    dispatch(state, msg).expect("a request carrying an id is answered")
}

fn code(res: &Value) -> i64 {
    res["error"]["code"].as_i64().expect("an error code")
}

fn message(res: &Value) -> &str {
    res["error"]["message"].as_str().expect("an error message")
}

fn server_info(result: &Value) -> &Value {
    &result["_meta"]["io.modelcontextprotocol/serverInfo"]
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
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(result["supportedVersions"], json!(SUPPORTED_REVISIONS));
    assert_eq!(
        result["capabilities"]["tools"],
        json!({ "listChanged": false })
    );
    assert_eq!(result["ttlMs"], json!(3_600_000));
    assert_eq!(result["cacheScope"], json!("public"));
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
fn optional_metadata_omitted_is_accepted() {
    let res = answer(
        &state(),
        &request("server/discover", &required_meta(CANDIDATE_REVISION)),
    );
    assert!(
        res.get("error").is_none(),
        "optional keys never reject: {res}"
    );
}

#[test]
fn optional_metadata_present_is_accepted() {
    let res = answer(
        &state(),
        &request("server/discover", &full_meta(CANDIDATE_REVISION)),
    );
    assert!(
        res.get("error").is_none(),
        "optional keys never reject: {res}"
    );
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
    let answers = json!({ "inputResponses": { "ask-1": { "action": "accept", "content": {} } } });
    let cases = [
        ("tools/call", None, None),
        ("tools/call", Some(json!({})), None),
        ("tools/call", Some(json!({ "inputResponses": {} })), None),
        ("tools/call", Some(answers.clone()), Some("elicitation")),
        ("tools/list", Some(answers.clone()), None),
        ("server/discover", Some(answers), None),
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
    let msg = tool_call(
        &required_meta(CANDIDATE_REVISION),
        json!({ "inputResponses": { "ask-1": { "action": "accept", "content": {} } } }),
    );
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
fn answering_input_requests_with_elicitation_succeeds() {
    let msg = tool_call(
        &eliciting_meta(CANDIDATE_REVISION),
        json!({ "inputResponses": { "ask-1": { "action": "accept", "content": {} } } }),
    );
    let res = answer(&state(), &msg);
    assert!(res.get("error").is_none(), "{res}");
    assert_eq!(res["result"]["isError"], json!(false), "{res}");
    assert_eq!(res["result"]["resultType"], json!("complete"));
    assert_eq!(
        server_info(&res["result"])["name"],
        json!(env!("CARGO_PKG_NAME"))
    );
}

#[test]
fn ordinary_tool_calls_need_no_client_capability() {
    let res = answer(
        &state(),
        &tool_call(&required_meta(CANDIDATE_REVISION), json!({})),
    );
    assert_ne!(res["error"]["code"], json!(-32021), "{res}");
    assert!(res.get("error").is_none(), "no invented requirement: {res}");
    assert_eq!(res["result"]["isError"], json!(false), "{res}");
}

#[test]
fn tools_list_serves_the_production_catalogue_for_the_revision() {
    let res = answer(
        &state(),
        &request("tools/list", &required_meta(CANDIDATE_REVISION)),
    );
    let result = &res["result"];
    assert_eq!(result["tools"], McpHandler::tools_list()["tools"]);
    assert!(
        !result["tools"].as_array().expect("a tool list").is_empty(),
        "the bound catalogue is present"
    );
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["ttlMs"], json!(0));
    assert_eq!(result["cacheScope"], json!("private"));
    assert_eq!(server_info(result)["name"], json!(env!("CARGO_PKG_NAME")));
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
}

#[test]
fn notifications_and_id_less_messages_are_not_answered() {
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
