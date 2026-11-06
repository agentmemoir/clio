// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Shared MCP protocol tests: initialize, tools/list, tools/call
//! envelope and error mapping, notifications, unknown methods.

use super::*;
use crate::bound_tools;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::json;
use std::sync::Arc;

fn handler() -> (McpHandler, Arc<McpState>) {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let handler = McpHandler::new(Arc::clone(&state));
    (handler, state)
}

fn init_msg() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}}
    })
}

#[test]
fn initialize_echoes_pinned_revision() {
    let (h, _) = handler();
    let res = h.handle_message(&init_msg()).expect("response");
    assert_eq!(res["jsonrpc"], json!("2.0"));
    assert_eq!(res["id"], json!(1));
    assert_eq!(
        res["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
    assert_eq!(
        res["result"]["serverInfo"]["name"],
        json!(env!("CARGO_PKG_NAME"))
    );
}

#[test]
fn t03_tools_list_returns_the_selected_profile() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list",
                "params":{"_meta":{"com.agentmemoir/pageSize":100}}}))
        .expect("response");
    let tools = res["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    let bound = bound_tools();
    let expected: Vec<&str> = clio_types::DEFAULT_PRESET
        .tools()
        .into_iter()
        .filter(|name| bound.contains(name))
        .collect();
    assert_eq!(names.len(), expected.len());
    for name in expected {
        assert!(names.contains(&name), "`{name}` missing from tools/list");
    }
    // Read tools are MCP-bound and inside the default profile.
    assert!(names.contains(&"retrieve"));
    // Every listed tool has an object inputSchema.
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], json!("object"));
    }
}

#[test]
fn tools_call_success_wraps_result() {
    let (h, state) = handler();
    state
        .ops_store
        .register_bank("bank-a", "2024-01-01T00:00:00Z")
        .expect("register empty bank fixture");
    let _ = h.handle_message(&init_msg());
    let res = h
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "consolidate", "arguments": {"bank": "bank-a"}}
        }))
        .expect("response");
    assert_eq!(res["result"]["isError"], json!(false));
    let payload = &res["result"]["structuredContent"];
    assert_eq!(payload["ok"], json!(true));
    let text = res["result"]["content"][0]["text"].as_str().expect("text");
    assert_eq!(text, "ok");
    let serialized_payload = serde_json::to_string(payload).expect("payload JSON");
    let raw = serde_json::to_string(&res).expect("response JSON");
    assert_eq!(
        raw.matches(&serialized_payload).count(),
        1,
        "payload must appear exactly once: {raw}"
    );
}

#[test]
fn schema_envelope_summarizes_status_and_carries_payload_once() {
    let payload = json!({"ok": true, "marker": "payload-once-8f3a", "items": [1, 2, 3]});
    let envelope = tool_call_envelope(&payload, true);
    assert_eq!(envelope["structuredContent"], payload);
    assert_eq!(envelope["content"][0]["text"], json!("ok"));
    assert_eq!(envelope["isError"], json!(false));
    let serialized_payload = serde_json::to_string(&payload).expect("payload JSON");
    let raw = serde_json::to_string(&envelope).expect("envelope JSON");
    assert_eq!(
        raw.matches(&serialized_payload).count(),
        1,
        "payload must appear exactly once: {raw}"
    );
}

#[test]
fn schema_envelope_success_text_is_status_only() {
    let payload = json!({"ok": true, "marker": "success-text-6d41", "items": [7, 8]});
    let envelope = tool_call_envelope(&payload, true);
    let text = envelope["content"][0]["text"].as_str().expect("text");
    assert_eq!(text, "ok");
    assert!(
        !text.contains("success-text-6d41"),
        "text must not carry the payload: {text}"
    );
}

#[test]
fn schema_envelope_error_text_is_code_and_message_only() {
    let payload = json!({
        "ok": false,
        "code": "invalid_argument",
        "message": "bad value",
        "detail": "error-payload-3e59"
    });
    let envelope = tool_call_envelope(&payload, true);
    assert_eq!(envelope["isError"], json!(true));
    let text = envelope["content"][0]["text"].as_str().expect("text");
    assert_eq!(text, "error: invalid_argument: bad value");
    assert!(
        !text.contains("error-payload-3e59"),
        "text must not carry the payload: {text}"
    );
}

#[test]
fn no_schema_envelope_keeps_payload_in_content_only() {
    let payload = json!({"ok": true, "marker": "no-schema-2c1d"});
    let envelope = tool_call_envelope(&payload, false);
    assert!(
        envelope.get("structuredContent").is_none(),
        "no-schema envelope must omit structuredContent: {envelope}"
    );
    assert_eq!(envelope["isError"], json!(false));
    let text = envelope["content"][0]["text"].as_str().expect("text");
    let parsed: Value = serde_json::from_str(text).expect("text is JSON");
    assert_eq!(parsed, payload);
}

#[test]
fn error_summary_drops_missing_parts() {
    assert_eq!(
        error_summary(&json!({"ok": false, "code": "invalid_argument", "message": "bad"})),
        "error: invalid_argument: bad"
    );
    assert_eq!(
        error_summary(&json!({"ok": false, "code": "invalid_argument"})),
        "error: invalid_argument"
    );
    assert_eq!(
        error_summary(&json!({"ok": false, "message": "bad"})),
        "error: bad"
    );
    assert_eq!(error_summary(&json!({"ok": false})), "error");
}

#[test]
fn t05_gated_reject_maps_to_tool_error_result() {
    let (h, _) = handler();
    let res = h.handle_message(&json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "store", "arguments": {"item": {"id": "x"}, "epistemic_kind": "fact"}}
    })).expect("response");
    assert_eq!(res["result"]["isError"], json!(true));
    let structured = &res["result"]["structuredContent"];
    assert_eq!(structured["ok"], json!(false));
    assert_eq!(structured["code"], json!("invalid_argument"));
    let text = res["result"]["content"][0]["text"].as_str().expect("text");
    let expected = format!(
        "error: {}: {}",
        structured["code"].as_str().expect("code"),
        structured["message"].as_str().expect("message")
    );
    assert_eq!(text, expected);
    assert!(
        !text.contains('{'),
        "text must not carry the payload: {text}"
    );
}

#[test]
fn t11_wrong_update_rule_maps_to_is_error() {
    let (h, _) = handler();
    let res = h.handle_message(&json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "update", "arguments": {"target": "a", "new_value": "1", "update_rule": "fact"}}
    })).expect("response");
    assert_eq!(res["result"]["isError"], json!(true));
    assert_eq!(
        res["result"]["structuredContent"]["code"],
        json!("wrong_update_rule")
    );
}

#[test]
fn unknown_tool_is_invalid_params() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "no_such_tool", "arguments": {}}
        }))
        .expect("response");
    assert_eq!(res["error"]["code"], json!(ERR_INVALID_PARAMS));
    assert!(
        res["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no_such_tool")
    );
}

#[test]
fn unknown_method_is_method_not_found() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({"jsonrpc":"2.0","id":7,"method":"resources/list"}))
        .expect("response");
    assert_eq!(res["error"]["code"], json!(ERR_METHOD_NOT_FOUND));
}

#[test]
fn ping_and_notifications() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({"jsonrpc":"2.0","id":8,"method":"ping"}))
        .expect("response");
    assert_eq!(res["result"], json!({}));
    // Notifications get no response.
    assert!(
        h.handle_message(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none()
    );
    assert!(
        h.handle_message(&json!({"jsonrpc":"2.0","method":"notifications/unknown"}))
            .is_none()
    );
    // Malformed frame maps to a parse error with null id.
    let res = h.handle_frame("not json").expect("response");
    assert_eq!(res["error"]["code"], json!(ERR_PARSE));
    assert!(res["id"].is_null());
}

#[test]
fn missing_method_is_invalid_request() {
    let (h, _) = handler();
    let res = h
        .handle_frame(r#"{"jsonrpc":"2.0","id":1}"#)
        .expect("response");
    assert_eq!(res["error"]["code"], json!(ERR_INVALID_REQUEST));
}

#[test]
fn tools_call_with_missing_params_rejected() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call"}))
        .expect("response");
    assert_eq!(res["error"]["code"], json!(ERR_INVALID_PARAMS));
}

#[test]
fn handler_accessors_and_default_arguments() {
    let (h, state) = handler();
    assert_eq!(h.protocol_revision(), MCP_PROTOCOL_REVISION);
    assert!(Arc::ptr_eq(h.state(), &state));
    // tools/call without `arguments` uses an empty arg map.
    let res = h
        .handle_message(&json!({
            "jsonrpc":"2.0","id":10,"method":"tools/call",
            "params":{"name":"consolidate"}
        }))
        .expect("response");
    assert_eq!(res["result"]["isError"], json!(false));
}

#[test]
fn initialize_reports_pin_for_mismatched_client_revision() {
    let (h, _) = handler();
    let res = h
        .handle_message(&json!({
            "jsonrpc":"2.0","id":11,"method":"initialize",
            "params":{"protocolVersion":"2026-07-28","capabilities":{}}
        }))
        .expect("response");
    assert_eq!(
        res["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
}

/// A co-activation handoff failure maps to JSON-RPC `-32001` with the
/// retryable structured payload; other results map to `None`.
#[test]
fn coactivation_handoff_failure_maps_to_32001() {
    let failure = json!({"ok": false, "code": "coactivation_handoff_failed", "message": "x"});
    let err = coactivation_rpc_error(&failure).expect("mapped");
    let response = McpHandler::error_response(Some(&json!(1)), &err);
    assert_eq!(response["error"]["code"], json!(ERR_COACTIVATION_HANDOFF));
    assert_eq!(
        response["error"]["data"]["error"],
        json!("COACTIVATION_HANDOFF_FAILED")
    );
    assert_eq!(response["error"]["data"]["retryable"], json!(true));
    assert_eq!(response["error"]["data"]["query_executed"], json!(false));
    assert!(coactivation_rpc_error(&json!({"ok": true})).is_none());
}

/// Every newly bound tool answers a full `tools/call` request/response through
/// the shared handler (MCP envelope with `isError` / `structuredContent`), not
/// just the in-process dispatcher seam.
#[test]
fn new_tools_round_trip_over_tools_call() {
    let (h, _) = handler();
    let success: [(&str, Value); 6] = [
        ("config_get", json!({})),
        ("config_profiles", json!({})),
        ("ranking_env_get", json!({})),
        (
            "config_set",
            json!({"path": "log.level", "value": "debug", "scope": "session"}),
        ),
        ("config_profile_apply", json!({"name": "coding_local"})),
        (
            "ranking_env_set",
            json!({"patch": {"theta_admit": 0.4}, "dry_run": true}),
        ),
    ];
    for (name, args) in success {
        let res = h
            .handle_message(&json!({
                "jsonrpc": "2.0", "id": 20, "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }))
            .expect("response");
        assert_eq!(res["result"]["isError"], json!(false), "{name}: {res}");
        assert_eq!(
            res["result"]["structuredContent"]["ok"],
            json!(true),
            "{name}: {res}"
        );
        let text = res["result"]["content"][0]["text"].as_str().expect("text");
        assert_eq!(text, "ok", "{name}: {res}");
    }
    // `summarize` is bound but needs a configured extraction provider; without
    // one the envelope still round-trips and fails closed with a typed code.
    let res = h
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 21, "method": "tools/call",
            "params": {"name": "summarize", "arguments": {"scope": "bank"}}
        }))
        .expect("response");
    assert_eq!(res["result"]["isError"], json!(true), "{res}");
    assert_eq!(
        res["result"]["structuredContent"]["code"],
        json!("not_implemented"),
        "{res}"
    );
}

/// Every name the pack advertises as bound reaches a dispatch handler: a bound
/// tool must not fall through to the catch-all that names an unhandled tool.
///
/// `tools/call` first rejects any name outside `bound_tools()`, so a protocol
/// call can never reach that gap; this calls the dispatcher directly. The
/// provider-gated tool (`summarize`) legitimately answers `not_implemented`
/// with its own reason, so the assertion is on the catch-all's message, not on
/// the code. `nfr7_every_published_tool_dispatches_through_its_schema` makes
/// the same check over the published pack with minimal arguments.
#[test]
fn every_bound_tool_is_routable() {
    let (_, state) = handler();
    for name in bound_tools() {
        let result = crate::write_tools::dispatch(&state, name, &json!({}));
        let message = result["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("is declared but not implemented in this build"),
            "bound tool `{name}` has no dispatch handler: {result}"
        );
    }
}
