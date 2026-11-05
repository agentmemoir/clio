// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate
//! `2026-07-28` dispatcher: metadata validation, error codes, discovery,
//! removal of the handshake, and the guard that keeps the pin where it is.
//!
//! ## Owns
//! - Every assertion about `dispatch` and the revision constants
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

use super::{CANDIDATE_REVISION, McpHandler, SUPPORTED_REVISIONS, dispatch};
use serde_json::{Value, json};

/// Request metadata carrying the two keys the revision requires.
fn required_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {}
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

fn answer(msg: &Value) -> Value {
    dispatch(msg).expect("a request carrying an id is answered")
}

fn code(res: &Value) -> i64 {
    res["error"]["code"].as_i64().expect("an error code")
}

fn message(res: &Value) -> &str {
    res["error"]["message"].as_str().expect("an error message")
}

#[test]
fn discover_reports_versions_capabilities_and_identity() {
    let res = answer(&request(
        "server/discover",
        &required_meta(CANDIDATE_REVISION),
    ));
    let result = &res["result"];
    assert_eq!(res["jsonrpc"], json!("2.0"));
    assert_eq!(res["id"], json!(7));
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["supportedVersions"], json!(SUPPORTED_REVISIONS));
    assert_eq!(
        result["capabilities"]["tools"],
        json!({ "listChanged": false })
    );
    assert_eq!(result["ttlMs"], json!(3_600_000));
    assert_eq!(result["cacheScope"], json!("public"));
}

#[test]
fn discover_identity_sits_in_result_meta() {
    let res = answer(&request(
        "server/discover",
        &required_meta(CANDIDATE_REVISION),
    ));
    let result = &res["result"];
    let info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    assert_eq!(info["name"], json!(env!("CARGO_PKG_NAME")));
    assert_eq!(info["version"], json!(env!("CARGO_PKG_VERSION")));
    assert!(
        result.get("serverInfo").is_none(),
        "identity is not a result field"
    );
}

#[test]
fn absent_meta_is_invalid_params() {
    let res = answer(&json!({
        "jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": {}
    }));
    assert_eq!(code(&res), -32602);
    assert!(
        message(&res).contains("_meta"),
        "names the absent key: {res}"
    );
}

#[test]
fn absent_protocol_version_is_invalid_params() {
    let meta = json!({ "io.modelcontextprotocol/clientCapabilities": {} });
    let res = answer(&request("server/discover", &meta));
    assert_eq!(code(&res), -32602);
    assert!(
        message(&res).contains("protocolVersion"),
        "names the key: {res}"
    );
}

#[test]
fn non_string_protocol_version_is_invalid_params() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": ["2026-07-28"],
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let res = answer(&request("server/discover", &meta));
    assert_eq!(code(&res), -32602);
    assert!(
        message(&res).contains("protocolVersion"),
        "names the key: {res}"
    );
}

#[test]
fn absent_client_capabilities_is_invalid_params() {
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": CANDIDATE_REVISION });
    let res = answer(&request("server/discover", &meta));
    assert_eq!(code(&res), -32602);
    assert!(
        message(&res).contains("clientCapabilities"),
        "names the key: {res}"
    );
}

#[test]
fn non_object_client_capabilities_is_invalid_params() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": CANDIDATE_REVISION,
        "io.modelcontextprotocol/clientCapabilities": []
    });
    let res = answer(&request("server/discover", &meta));
    assert_eq!(code(&res), -32602);
    assert!(
        message(&res).contains("clientCapabilities"),
        "names the key: {res}"
    );
}

#[test]
fn optional_metadata_present_is_accepted() {
    let res = answer(&request("server/discover", &full_meta(CANDIDATE_REVISION)));
    assert!(
        res.get("error").is_none(),
        "optional keys never reject: {res}"
    );
}

#[test]
fn empty_client_capabilities_suffices_for_a_catalogue_request() {
    let res = answer(&request("tools/list", &required_meta(CANDIDATE_REVISION)));
    assert!(
        res.get("error").is_none(),
        "no capability is gated yet: {res}"
    );
}

#[test]
fn unsupported_revision_reports_the_supported_list() {
    let res = answer(&request("server/discover", &required_meta("1900-01-01")));
    assert_eq!(code(&res), -32022);
    let data = &res["error"]["data"];
    assert_eq!(data["requested"], json!("1900-01-01"));
    let supported = data["supported"].as_array().expect("a supported list");
    assert!(supported.contains(&json!(CANDIDATE_REVISION)));
    assert!(supported.contains(&json!(crate::MCP_PROTOCOL_REVISION)));
}

#[test]
fn removed_initialize_is_an_unknown_method() {
    let res = answer(&json!({
        "jsonrpc": "2.0", "id": 7, "method": "initialize",
        "params": { "protocolVersion": CANDIDATE_REVISION, "capabilities": {} }
    }));
    assert_eq!(code(&res), -32601);
    assert!(
        message(&res).contains("initialize"),
        "names the method: {res}"
    );
}

#[test]
fn removed_ping_is_an_unknown_method() {
    let res = answer(&request("ping", &required_meta(CANDIDATE_REVISION)));
    assert_eq!(code(&res), -32601);
}

#[test]
fn removed_logging_set_level_is_an_unknown_method() {
    let res = answer(&request(
        "logging/setLevel",
        &required_meta(CANDIDATE_REVISION),
    ));
    assert_eq!(code(&res), -32601);
}

#[test]
fn unknown_method_is_an_unknown_method() {
    let res = answer(&request(
        "resources/list",
        &required_meta(CANDIDATE_REVISION),
    ));
    assert_eq!(code(&res), -32601);
}

#[test]
fn missing_method_is_invalid_request() {
    let res = answer(&json!({ "jsonrpc": "2.0", "id": 7, "params": {} }));
    assert_eq!(code(&res), -32600);
}

#[test]
fn initialized_notification_gets_no_response() {
    assert!(
        dispatch(&json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))
        .is_none()
    );
}

#[test]
fn message_without_id_gets_no_response() {
    let msg = json!({
        "jsonrpc": "2.0", "method": "tools/list", "params": { "_meta": required_meta(CANDIDATE_REVISION) }
    });
    assert!(dispatch(&msg).is_none());
}

#[test]
fn tools_list_serves_the_production_catalogue() {
    let res = answer(&request("tools/list", &required_meta(CANDIDATE_REVISION)));
    let result = &res["result"];
    assert_eq!(result["tools"], McpHandler::tools_list()["tools"]);
    assert!(
        !result["tools"].as_array().expect("a tool list").is_empty(),
        "the bound catalogue is present"
    );
}

#[test]
fn tools_list_carries_the_required_result_fields() {
    let res = answer(&request("tools/list", &required_meta(CANDIDATE_REVISION)));
    let result = &res["result"];
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["ttlMs"], json!(0));
    assert_eq!(result["cacheScope"], json!("private"));
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!(env!("CARGO_PKG_NAME"))
    );
}

#[test]
fn tools_call_is_refused_without_runtime_state() {
    let msg = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {
            "_meta": required_meta(CANDIDATE_REVISION),
            "name": "retrieve",
            "arguments": {}
        }
    });
    let res = answer(&msg);
    assert_eq!(code(&res), -32603);
    assert!(
        message(&res).contains("tools/call"),
        "names the method: {res}"
    );
}

#[test]
fn the_pin_is_untouched_by_the_candidate() {
    assert_eq!(crate::MCP_PROTOCOL_REVISION, "2025-11-25");
    assert_eq!(
        crate::schema::schema_pack()["mcp_protocol_revision"],
        json!("2025-11-25")
    );
    assert_eq!(SUPPORTED_REVISIONS[0], CANDIDATE_REVISION);
    assert_ne!(CANDIDATE_REVISION, crate::MCP_PROTOCOL_REVISION);
}
