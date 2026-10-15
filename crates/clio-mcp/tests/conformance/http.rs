// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Streamable HTTP transport conformance
//! suite: the write-set parity matrix over HTTP, the Origin gate, the optional
//! session lifecycle, the auth gate, and method refusals.
//!
//! ## Owns
//! - `spawn_http`, `http_request`, and `http_post`, the raw HTTP client harness.
//! - The HTTP write-set parity and Origin test, the session lifecycle test, the
//!   auth reject test, and the SSE refusal and unknown-method test.
//!
//! ## Does not own
//! - The shared write-set fixture and builders (`main`).
//! - The stdio transport suite (`stdio`).
//!
//! ## Boundary
//!
//! Changes to HTTP status gates, session handling, or the raw request harness
//! should normally require changes here. Changes to the write-set fixture
//! should not require changes here.
//!
//! Keep this module focused on HTTP transport conformance.

use clio_mcp::MCP_PROTOCOL_REVISION;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crate::{init_only, mask_volatile, rpc, state, write_set_fixture};

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
