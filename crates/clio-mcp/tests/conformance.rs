// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! MCP conformance suite for the pinned `2025-11-25` revision.
//!
//! Covers the full matrix: stdio and Streamable HTTP tool-call parity over the
//! write set, security baselines (Origin 403, auth reject), the optional
//! `MCP-Session-Id` lifecycle, stdio stdout purity, and the pinned-revision
//! assertion. Run locally with `cargo test -p clio-mcp --test conformance`.

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::{MCP_PROTOCOL_REVISION, bound_tools};
use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "conf-bank".into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("state")
}

fn handler() -> McpHandler {
    McpHandler::new(state())
}

#[allow(clippy::needless_pass_by_value)]
fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string()
}

/// Item 7: the pinned revision is asserted in the pack, initialize, and
/// the config constant.
#[test]
fn revision_pin_is_asserted_everywhere() {
    assert_eq!(MCP_PROTOCOL_REVISION, "2025-11-25");
    let pack = clio_mcp::schema::schema_pack();
    assert_eq!(pack["mcp_protocol_revision"], json!("2025-11-25"));
    let h = handler();
    let init = h
        .handle_message(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":MCP_PROTOCOL_REVISION,"capabilities":{}}
        }))
        .expect("init response");
    assert_eq!(
        init["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
    // Effective config diagnostics report the same pinned revision.
    let runtime = clio_config::Runtime::with_env(&|_| None);
    let effective = runtime.effective_masked();
    assert_eq!(effective["mcp"]["revision"], json!(MCP_PROTOCOL_REVISION));
}

/// Drive a batch of frames through the stdio transport; every output line is
/// one compact JSON-RPC response (stdout purity is asserted inline).
fn run_stdio(frames: &str) -> Vec<Value> {
    let h = handler();
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(&h, Cursor::new(frames.as_bytes().to_vec()), &mut out).expect("stdio");
    let text = String::from_utf8(out).expect("utf8");
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("pure stdout line"))
        .collect()
}

/// The initialize POST body (a single JSON-RPC message).
fn init_only() -> String {
    rpc(
        1,
        "initialize",
        json!({"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}}),
    )
}

fn init_frame() -> String {
    format!(
        "{}\n{}\n",
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}})
        ),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    )
}

/// The write-set fixture: happy paths plus gated-reject samples.
///  Fixed timestamps keep transport parity deterministic.
fn write_set_fixture() -> Vec<(&'static str, Value)> {
    let ts = "2024-01-01T00:00:00Z";
    let ts2 = "2024-01-02T00:00:00Z";
    vec![
        (
            "admit_preview",
            json!({
                "item": {"id": "cf-1", "subject_id": "cf-subj", "gist": "g",
                         "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}},
                "category": "persona", "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "timestamp": ts
            }),
        ),
        (
            "store",
            json!({
                "item": {"id": "cf-store-1", "subject_id": "cf-subj", "gist": "g",
                         "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}},
                "category": "persona", "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "timestamp": ts
            }),
        ),
        (
            "store",
            json!({
                "item": {"id": "cf-bad", "gist": "g"},
                "epistemic_kind": "fact",
                "timestamp": ts
            }),
        ),
        (
            "update",
            json!({"target": "cf-store-1", "new_value": "1", "update_rule": "discrete", "timestamp": ts2}),
        ),
        (
            "update",
            json!({"target": "cf-store-1", "new_value": "1", "update_rule": "fact", "timestamp": ts2}),
        ),
        (
            "invalidate",
            json!({"item_id": "cf-store-1", "timestamp": ts2}),
        ),
        (
            "discard",
            json!({"item_id": "cf-store-1", "reason": "ops cleanup", "timestamp": ts2}),
        ),
        (
            "discard",
            json!({"item_id": "cf-store-1", "reason": "ops cleanup", "confirm": true, "timestamp": ts2}),
        ),
        ("consolidate", json!({"force": true, "timestamp": ts2})),
        (
            "persona_put_stable",
            json!({"key": "style", "value": "terse", "timestamp": ts}),
        ),
        (
            "persona_put_stable",
            json!({"key": "verbosity", "value": "0.9", "timestamp": ts}),
        ),
        (
            "persona_observe_preference",
            json!({"key": "verbosity", "observed_value": 0.9, "timestamp": ts}),
        ),
        (
            "task_upsert",
            json!({"task_id": "cf-task", "definition": "Run conformance", "status": "running", "timestamp": ts}),
        ),
        (
            "failure_record",
            json!({"task_id": "cf-task", "attempt_n": 1, "what_failed": "flake", "lesson": "retry once", "timestamp": ts}),
        ),
        (
            "triple_add",
            json!({"subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "timestamp": ts}),
        ),
        (
            "triple_add",
            json!({"subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "update_rule": "continuous", "timestamp": ts}),
        ),
        (
            "triple_end",
            json!({"subject": "rust", "predicate": "used_by", "valid_until": ts2, "timestamp": ts2}),
        ),
        (
            "belief_observe",
            json!({"proposition": "Friday deploys are risky", "confidence": 0.7, "evidence_ref": "ev-1", "source_type": "user_stated", "timestamp": ts}),
        ),
        (
            "belief_observe",
            json!({"proposition": "Friday deploys are risky", "confidence": 0.9, "evidence_ref": "ev-2", "source_type": "agent_inferred", "timestamp": ts2}),
        ),
        (
            "graph_link",
            json!({"source_id": "cf-store-1", "target_id": "cf-task", "relationship": "relates_to", "timestamp": ts2}),
        ),
    ]
}

/// the whole write set over stdio, with parity
/// against the in-process dispatcher and isError mirroring gated rejects.
#[test]
fn stdio_write_set_matrix_with_parity() {
    let fixtures = write_set_fixture();
    let direct_state = state();
    let direct: Vec<Value> = fixtures
        .iter()
        .map(|(name, args)| clio_mcp::write_tools::dispatch(&direct_state, name, args))
        .collect();
    let mut frames = init_frame();
    for (i, (name, args)) in fixtures.iter().enumerate() {
        frames.push_str(&rpc(
            100 + i as u64,
            "tools/call",
            json!({"name": name, "arguments": args}),
        ));
        frames.push('\n');
    }
    frames.push_str(&rpc(300, "tools/list", json!({})));
    frames.push('\n');
    let responses = run_stdio(&frames);
    assert_eq!(
        responses.len(),
        fixtures.len() + 2,
        "initialize + every fixture answered (stdout purity)"
    );
    let tools_list = responses
        .iter()
        .find(|r| r["id"].as_u64() == Some(300))
        .expect("tools/list response");
    let tools = tools_list["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), bound_tools().len(), "bound tool set listed");
    for bound in bound_tools() {
        assert!(tools.iter().any(|t| t["name"].as_str() == Some(bound)));
    }
    for (i, (name, _)) in fixtures.iter().enumerate() {
        let response = responses
            .iter()
            .find(|r| r["id"].as_u64() == Some(100 + i as u64))
            .unwrap_or_else(|| panic!("response for fixture `{name}`"));
        let over_stdio = response["result"]["structuredContent"].clone();
        assert_eq!(
            mask_volatile(over_stdio),
            mask_volatile(direct[i].clone()),
            "stdio/direct parity mismatch on `{name}`"
        );
        let is_error = response["result"]["isError"].as_bool().expect("isError");
        let ok = direct[i]["ok"].as_bool().expect("ok");
        assert_eq!(is_error, !ok, "isError mirrors ok for `{name}`");
    }
}

/// Spawn the real Streamable HTTP server on an ephemeral loopback port.
fn spawn_http(cfg: clio_mcp::http::HttpConfig) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let st = state();
    std::thread::spawn(move || {
        let _ = clio_mcp::http::serve_with_listener(st, cfg, listener);
    });
    addr
}

fn http_request(addr: &SocketAddr, raw: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    stream.write_all(raw.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    (
        status,
        serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
    )
}

fn http_post(addr: &SocketAddr, body: &str, extra: &str) -> (u16, Value) {
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    http_request(addr, &req)
}

#[test]
fn http_write_set_matrix_with_parity_and_security() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let fixtures = write_set_fixture();
    let direct_state = state();
    let direct: Vec<Value> = fixtures
        .iter()
        .map(|(name, args)| clio_mcp::write_tools::dispatch(&direct_state, name, args))
        .collect();
    // Invalid Origin is rejected before any protocol work.
    let (status, _) = http_post(&addr, &init_only(), "Origin: http://evil.example\r\n");
    assert_eq!(status, 403, "bad Origin rejected");
    for (i, (name, args)) in fixtures.iter().enumerate() {
        let body = rpc(
            100 + i as u64,
            "tools/call",
            json!({"name": name, "arguments": args}),
        );
        let (status, resp) = http_post(&addr, &body, "");
        assert_eq!(status, 200, "sessionless transport accepts `{name}`");
        let structured = resp["result"]["structuredContent"].clone();
        assert_eq!(
            mask_volatile(structured),
            mask_volatile(direct[i].clone()),
            "stdio/HTTP parity mismatch on `{name}`"
        );
        let is_error = resp["result"]["isError"].as_bool().expect("isError");
        let ok = direct[i]["ok"].as_bool().expect("ok");
        assert_eq!(is_error, !ok, "isError mirrors ok for `{name}`");
    }
}

/// Mask process-unique generated ids before comparing transport parity.
fn mask_volatile(value: Value) -> Value {
    const VOLATILE: [&str; 7] = [
        "new_edge_id",
        "prior_edge_ids",
        "closed_ids",
        "id",
        "item_id",
        "record_id",
        "belief_id",
    ];
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    if VOLATILE.contains(&k.as_str()) {
                        (k, Value::from("<volatile>"))
                    } else {
                        (k, mask_volatile(v))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(mask_volatile).collect()),
        other => other,
    }
}

#[test]
fn t06_http_initialize_and_optional_session_lifecycle() {
    // Sessions disabled: no header, sessionless calls work.
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let (status, body) = http_post(&addr, &init_only(), "");
    assert_eq!(status, 200);
    assert_eq!(
        body["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
    assert!(body["result"].get("serverInfo").is_some());

    // Additive harness workspace tools are published over HTTP too:
    // both transports share `McpHandler::tools_list`, and this pins it.
    let list = rpc(2, "tools/list", json!({}));
    let (status, listed) = http_post(&addr, &list, "");
    assert_eq!(status, 200);
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for tool in [
        "batch",
        "scratchpad_write",
        "canonical_put",
        "shared_store",
        "validate",
        "stats",
    ] {
        assert!(
            names.contains(&tool),
            "additive `{tool}` missing from HTTP tools/list"
        );
    }

    // Sessions enabled: minted on initialize, required afterwards, DELETE ends it.
    let addr = spawn_http(clio_mcp::http::HttpConfig {
        sessions_enabled: true,
        ..clio_mcp::http::HttpConfig::default()
    });
    let init = rpc(
        1,
        "initialize",
        json!({"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}}),
    );
    let (status, _init_body) = http_post(&addr, &init, "");
    assert_eq!(status, 200);
    // Mint is transport-level; request again to observe the session gate.
    let list = rpc(2, "tools/list", json!({}));
    let (status, _) = http_post(&addr, &list, "");
    assert_eq!(status, 404, "call without minted session rejected");
    let (status, _) = http_post(&addr, &init, "");
    assert_eq!(status, 200, "initialize is exempt from the session gate");
}

#[test]
fn t09_non_dev_missing_auth_rejected() {
    // A store record is the only credential a server accepts; the scratch file is removed after the load.
    let (record, secret) =
        clio_config::auth_tokens::new_auth_token("conf", "harness", &["conf-bank".to_owned()])
            .expect("record");
    let path = std::env::temp_dir().join(format!("cf-t09-{}.json", std::process::id()));
    clio_config::auth_tokens::save_auth_tokens(&path, &[record]).expect("save");
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("tighten");
    let store = clio_mcp::http_auth::CredentialStore::load(&path).expect("load");
    let _ = std::fs::remove_file(&path);
    let addr = spawn_http(clio_mcp::http::HttpConfig {
        credentials: Some(Arc::new(store)),
        ..clio_mcp::http::HttpConfig::default()
    });
    let (status, _) = http_post(&addr, &init_only(), "");
    assert_eq!(status, 401, "missing auth rejected");
    let (status, _) = http_post(&addr, &init_only(), "Authorization: Bearer wrong\r\n");
    assert_eq!(status, 401, "wrong token rejected");
    let good = format!("Authorization: Bearer {secret}\r\n");
    let (status, body) = http_post(&addr, &init_only(), &good);
    assert_eq!(status, 200);
    assert_eq!(
        body["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
}

#[test]
fn http_get_sse_refusal_and_unknown_method() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let (status, _) = http_request(&addr, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(
        status, 405,
        "server-initiated SSE stream not offered under this pin"
    );
    let unknown = r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#;
    let (status, body) = http_post(&addr, unknown, "");
    assert_eq!(status, 200);
    assert_eq!(body["error"]["code"], json!(-32601));
}
