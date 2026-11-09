// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the wire half of the binding harness: how one
//! JSON-RPC request is framed, which conditional headers it must carry, and the
//! raw `POST /mcp` exchange that sends it.
//!
//! ## Does not own
//! - The credential store, the servers, or the result predicates (`super`)
//! - The transport's rules (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a request is framed or sent should normally require changes
//! here. Adding a scenario should not.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The conditional headers one request body must carry: the revision it is
/// using and the method it names, plus the resource name a tool call names.
pub fn conditional_headers(body: &str) -> String {
    let frame: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let method = frame["method"].as_str().unwrap_or_default();
    let name = frame
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let revision = clio_mcp::MCP_PROTOCOL_REVISION;
    format!("Mcp-Protocol-Version: {revision}\r\nMcp-Method: {method}\r\nMcp-Name: {name}\r\n")
}

/// One `POST /mcp` and the status plus body that came back.
///
/// The write side is half-closed once the sent, so the server's read of the
/// declared body completes rather than waiting for a close.
pub fn post(port: u16, body: &str, extra: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n{}{extra}\r\n{body}",
        body.len(),
        conditional_headers(body)
    );
    stream.write_all(request.as_bytes()).expect("write");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos.min(buf.len())]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let from = (pos + 4).min(buf.len());
    (status, String::from_utf8_lossy(&buf[from..]).into_owned())
}

/// The `Authorization` header carrying `secret`.
pub fn bearer(secret: &str) -> String {
    format!("Authorization: Bearer {secret}\r\n")
}

/// One JSON-RPC `tools/call` frame for `tool` with `arguments`.
#[allow(clippy::needless_pass_by_value)]
pub fn call(tool: &str, arguments: Value) -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": clio_mcp::MCP_PROTOCOL_REVISION,
                "io.modelcontextprotocol/clientCapabilities": {}
            },
            "name": tool, "arguments": arguments
        }
    })
    .to_string()
}
