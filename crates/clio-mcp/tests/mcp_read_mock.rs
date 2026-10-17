// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Mock MCP client harness for the read surface: simulates dropped connections,
//! mid-stream aborts, and dropped `MCP-Session-Id` tokens, then verifies the
//! read dispatcher keeps state consistent (no ghost reads, no corrupt edges).

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

const BANK: &str = "mock-bank";
const TS: &str = "2024-01-01T00:00:00Z";

fn seed_state() -> Arc<McpState> {
    let state = McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("state");
    // Distinct gists per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    for (id, category, gist) in [
        ("m-1", "task_spec", "terse answers m-1"),
        ("m-2", "persona", "terse answers m-2"),
    ] {
        let out = clio_mcp::write_tools::dispatch(
            &state,
            "store",
            &json!({
                "item": {"id": id, "subject_id": "subj", "gist": gist,
                         "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}},
                "category": category, "epistemic_kind": "fact",
                "source_text": format!("The user prefers {gist} (1 note since 2024-01-01)."),
                "bank": BANK, "actor": "agent", "timestamp": TS
            }),
        );
        assert_eq!(out["ok"], json!(true), "seed {id}: {out}");
        state
            .index
            .put_lexical_doc(BANK, id, gist)
            .expect("lexical doc");
    }
    state
}

/// In-process mock client with an explicit connection lifecycle.
struct MockMcpClient {
    handler: McpHandler,
    state: Arc<McpState>,
    connected: bool,
    reconnects: usize,
}

impl MockMcpClient {
    fn connect(state: Arc<McpState>) -> Self {
        Self {
            handler: McpHandler::new(Arc::clone(&state)),
            state,
            connected: true,
            reconnects: 0,
        }
    }

    fn reconnect(&mut self) {
        self.connected = true;
        self.reconnects += 1;
        let _ = self.handler.handle_message(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": clio_mcp::MCP_PROTOCOL_REVISION, "capabilities": {}}
        }));
    }

    /// Simulate a transport drop; the next call transparently reconnects.
    fn drop_connection(&mut self) {
        self.connected = false;
    }

    #[allow(clippy::needless_pass_by_value)]
    fn call(&mut self, name: &str, args: Value) -> Value {
        if !self.connected {
            self.reconnect();
        }
        let response = self
            .handler
            .handle_message(&json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }))
            .expect("response");
        response["result"]["structuredContent"].clone()
    }

    /// Simulate a mid-stream abort: the in-flight call is discarded and the
    /// client reconnects before the next read.
    fn abort_midstream(&mut self) {
        self.connected = false;
    }
}

#[test]
fn mock_client_reconnect_keeps_read_consistency() {
    let mut client = MockMcpClient::connect(seed_state());
    let first = client.call("get", json!({"item_id": "m-1"}));
    client.drop_connection();
    let second = client.call("get", json!({"item_id": "m-1"}));
    assert_eq!(first, second, "reconnect must not change reads");
    assert_eq!(client.reconnects, 1);
    assert!(client.state.leaves.get(BANK, "m-1").is_some());
}

#[test]
fn mock_client_midstream_abort_leaves_no_ghost_reads() {
    let mut client = MockMcpClient::connect(seed_state());
    let before = client.call("inspect", json!({}));
    client.abort_midstream();
    let after = client.call("inspect", json!({}));
    assert_eq!(
        before["items"].as_array().expect("items").len(),
        after["items"].as_array().expect("items").len(),
        "an aborted stream must not create ghost items"
    );
    let missing = client.call("get", json!({"item_id": "ghost"}));
    assert_eq!(missing["code"], json!("item_not_found"));
}

#[test]
fn mock_client_abort_does_not_corrupt_assoc_transactions() {
    let mut client = MockMcpClient::connect(seed_state());
    let first = client.call("retrieve", json!({"query": "terse answers"}));
    assert_eq!(first["ok"], json!(true), "{first}");
    let edges_after_first = client
        .state
        .store
        .list_assoc_edges(BANK, None)
        .expect("edges")
        .len();
    client.abort_midstream();
    let second = client.call("retrieve", json!({"query": "terse answers"}));
    assert_eq!(second["ok"], json!(true), "{second}");
    let edges_after_second = client
        .state
        .store
        .list_assoc_edges(BANK, None)
        .expect("edges")
        .len();
    assert_eq!(
        edges_after_first, edges_after_second,
        "re-running retrieve must not duplicate or corrupt edge rows"
    );
}

fn spawn_http_session_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let state = seed_state();
    std::thread::spawn(move || {
        let cfg = clio_mcp::http::HttpConfig {
            sessions_enabled: true,
            insecure_no_auth: true,
            ..clio_mcp::http::HttpConfig::default()
        };
        let _ = clio_mcp::http::serve_with_listener(state, cfg, listener);
    });
    addr
}

fn http_post(addr: &SocketAddr, body: &str, extra: &str) -> (u16, Value, HashMap<String, String>) {
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    stream.write_all(req.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let mut headers = HashMap::new();
    for line in String::from_utf8_lossy(&buf[..pos]).lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let status = String::from_utf8_lossy(&buf[..pos])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    (
        status,
        serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
        headers,
    )
}

#[test]
fn mock_client_dropped_session_requires_reinitialize() {
    let addr = spawn_http_session_server();
    let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
                      "params":{"name":"inspect","arguments":{}}})
    .to_string();
    // Dropped session token: the server rejects the read.
    let (status, _, _) = http_post(&addr, &call, "");
    assert_eq!(status, 404, "read without a known session must be rejected");
    // Re-initialize mints a fresh session; the read then succeeds.
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                      "params":{"protocolVersion":clio_mcp::MCP_PROTOCOL_REVISION,"capabilities":{}}})
    .to_string();
    let (status, _, headers) = http_post(&addr, &init, "");
    assert_eq!(status, 200);
    let session = headers
        .get("mcp-session-id")
        .expect("initialize must mint a session id")
        .clone();
    let (status, body, _) = http_post(&addr, &call, &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(status, 200, "session read succeeds");
    assert_eq!(body["result"]["structuredContent"]["ok"], json!(true));
}

#[test]
fn mock_client_midstream_socket_close_then_clean_read() {
    let addr = spawn_http_session_server();
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                      "params":{"protocolVersion":clio_mcp::MCP_PROTOCOL_REVISION,"capabilities":{}}})
    .to_string();
    let (status, _, headers) = http_post(&addr, &init, "");
    assert_eq!(status, 200);
    let session = headers.get("mcp-session-id").expect("session").clone();

    // Abort mid-stream: send the request and drop the socket before reading.
    let call = json!({"jsonrpc":"2.0","id":8,"method":"tools/call",
                      "params":{"name":"get","arguments":{"item_id":"m-1"}}})
    .to_string();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nMCP-Session-Id: {session}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{call}",
        call.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream.write_all(req.as_bytes()).expect("write");
    drop(stream);

    // A fresh read on the same session still returns the correct item.
    let (status, body, _) = http_post(&addr, &call, &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(status, 200, "aborted stream must not wedge the session");
    let structured = &body["result"]["structuredContent"];
    assert_eq!(structured["ok"], json!(true), "{body}");
    assert_eq!(structured["id"], json!("m-1"));
}
