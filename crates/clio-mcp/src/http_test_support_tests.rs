// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared harness of the Streamable HTTP
//! transport unit tests: a live server on an OS-assigned port, raw request
//! writing, and response parsing.
//!
//! ## Owns
//! - `spawn_server` (the real `serve_with_listener` accept loop) and `INIT`
//! - `request`, `post_json`, and the `Response` parser
//!
//! ## Does not own
//! - Any scenario or assertion (the test modules own those)
//! - The transport's behavior (see `http` and `http_guard`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

/// Serve through the real `serve_with_listener` in a background thread; the
/// tests drive the production accept loop, not a test-local re-implementation.
///
/// The harness opts out of the start-time auth requirement (`insecure_no_auth`)
/// so a scenario that is not about the credential edge keeps the pre-existing
/// credential-free loopback server. Per-request auth still applies when the
/// caller's config carries a credential store.
pub(crate) fn spawn_server(cfg: HttpConfig) -> (u16, Arc<McpHandler>) {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let handler = Arc::new(McpHandler::new(Arc::clone(&state)));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let mut cfg = cfg;
        cfg.insecure_no_auth = true;
        let _ = serve_with_listener(state, cfg, listener);
    });
    (port, handler)
}

pub(crate) fn request(port: u16, raw: &str) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.read_to_end(&mut buf);
    parse_response(&buf)
}

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

pub(crate) fn parse_response(buf: &[u8]) -> Response {
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    Response {
        status,
        headers,
        body,
    }
}

pub(crate) fn post_json(port: u16, body: &str, extra: &str) -> Response {
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    request(port, &req)
}

pub(crate) const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
