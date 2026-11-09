// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `subscriptions/listen` scenario over a
//! real socket: the streamed acknowledgement, one frame per real tool-view
//! change, silence when nothing changed, and the closed stream that stops.
//!
//! ## Owns
//! - The raw listen request and the SSE reader over its response socket
//! - The change driver that applies a profile and announces the new view
//!
//! ## Does not own
//! - The envelopes or the streamed source (see `tool_listen`)
//! - The transport gates and write path (see `http`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::event_stream::frame;
use crate::http::HttpConfig;
use crate::http::http_test_support_tests::spawn_server;
use crate::http_candidate_fixtures::{
    body, check_error_envelope, headers as admission_headers, post,
};
use crate::runtime::McpState;
use crate::stateless_fixtures::{CANDIDATE_FIXTURE_REVISION, required_meta};
use crate::tool_listen::acknowledged;
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The body one listen request carries.
fn listen_frame(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "subscriptions/listen",
        "params": {
            "_meta": required_meta(CANDIDATE_FIXTURE_REVISION),
            "notifications": { "toolsListChanged": true }
        }
    })
}

/// The first position of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One SSE response socket and the bytes read but not yet split into frames.
struct Sse {
    stream: TcpStream,
    pending: Vec<u8>,
    timeout: Duration,
}

impl Sse {
    /// Read once; false on a timeout or a closed peer.
    fn read_more(&mut self) -> bool {
        let mut chunk = [0u8; 4096];
        self.stream
            .set_read_timeout(Some(self.timeout))
            .expect("a read timeout");
        match self.stream.read(&mut chunk) {
            Ok(0) => false,
            Ok(n) => {
                self.pending.extend_from_slice(&chunk[..n]);
                true
            }
            Err(ref err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                false
            }
            Err(_) => false,
        }
    }

    /// The next complete frame, or `None` when none arrived inside the timeout.
    fn frame(&mut self) -> Option<String> {
        loop {
            if let Some(pos) = find(&self.pending, b"\n\n") {
                let bytes: Vec<u8> = self.pending.drain(..pos + 2).collect();
                return Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            if !self.read_more() {
                return None;
            }
        }
    }
}

/// POST one listen request and return the response head and its stream.
fn open(port: u16, id: u64) -> (String, Sse) {
    let body = listen_frame(id).to_string();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{}\r\n{body}",
        body.len(),
        admission_headers("subscriptions/listen")
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    stream.write_all(request.as_bytes()).expect("write");

    let mut pending = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find(&pending, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).expect("the response head");
        assert!(n > 0, "the server answers the listen request");
        pending.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&pending[..head_end]).into_owned();
    let rest = pending[head_end + 4..].to_vec();
    (
        head,
        Sse {
            stream,
            pending: rest,
            timeout: Duration::from_secs(5),
        },
    )
}

/// The JSON value one SSE frame carries.
fn data(frame: &str) -> Value {
    let line = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("a frame carries one data line");
    serde_json::from_str(line).expect("a data line is JSON")
}

/// Select one of two distinct tool-group sets, then announce the view.
fn change_view(state: &McpState, all_groups: bool) -> bool {
    let groups = if all_groups {
        clio_types::ToolGroup::ALL.to_vec()
    } else {
        vec![clio_types::ToolGroup::Retrieval]
    };
    {
        let mut config = state.config.lock().expect("config mutex");
        config.set_mcp_tool_groups(groups);
    }
    state.announce_tool_view()
}

#[test]
fn a_listen_request_without_the_version_header_is_refused_before_a_stream_opens() {
    let (port, handler) = spawn_server(HttpConfig::default());
    let response = post(
        port,
        &listen_frame(1),
        "Mcp-Method: subscriptions/listen\r\n",
    );

    assert_eq!(response.status, 400, "{}", response.body);
    let answer = body(&response);
    check_error_envelope(&answer, 1).expect("a refusal is an error envelope");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("MCP-Protocol-Version")),
        "{answer}"
    );
    assert_eq!(
        handler.state().tool_signals.subscribers(),
        0,
        "a refused request registers no subscriber"
    );
}

#[test]
fn a_listen_request_whose_method_header_names_another_method_is_refused() {
    let (port, handler) = spawn_server(HttpConfig::default());
    let response = post(port, &listen_frame(1), &admission_headers("tools/list"));

    assert_eq!(response.status, 400, "{}", response.body);
    let answer = body(&response);
    check_error_envelope(&answer, 1).expect("a refusal is an error envelope");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Mcp-Method")),
        "{answer}"
    );
    assert_eq!(
        handler.state().tool_signals.subscribers(),
        0,
        "a refused request registers no subscriber"
    );
}

#[test]
fn a_listen_post_answers_with_an_event_stream_starting_at_the_acknowledgement() {
    let (port, _handler) = spawn_server(HttpConfig::default());
    let (head, mut sse) = open(port, 1);

    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("content-type: text/event-stream"),
        "the stream names its content type: {head}"
    );
    assert!(
        !lower.contains("content-length"),
        "a stream has no length: {head}"
    );
    assert!(
        !lower.contains("mcp-session-id"),
        "a subscription invents no session: {head}"
    );
    assert_eq!(
        sse.frame().expect("the acknowledgement is the first frame"),
        String::from_utf8(frame(&acknowledged(&json!(1), true).to_string()))
            .expect("a frame is UTF-8")
    );
}

#[test]
fn each_change_reaches_only_the_streams_that_are_still_open() {
    let (port, handler) = spawn_server(HttpConfig::default());
    let state = handler.state();
    let (_, mut live) = open(port, 1);
    let (_, mut closed) = open(port, 2);
    assert!(
        data(&live.frame().expect("the first acknowledgement"))["method"]
            .as_str()
            .is_some_and(|method| method == "notifications/subscriptions/acknowledged")
    );
    assert!(
        data(&closed.frame().expect("the second acknowledgement"))["method"]
            .as_str()
            .is_some_and(|method| method == "notifications/subscriptions/acknowledged")
    );

    assert!(change_view(state, true), "the first view change");
    for (id, sse) in [(1, &mut live), (2, &mut closed)] {
        let frame = sse.frame().expect("the change reaches the open stream");
        let value = data(&frame);
        assert_eq!(value["method"], json!("notifications/tools/list_changed"));
        assert_eq!(
            value["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            json!(id),
            "the frame names its own subscription"
        );
        sse.timeout = Duration::from_millis(300);
        assert!(
            sse.frame().is_none(),
            "one change sends one frame to subscription {id}"
        );
    }

    live.timeout = Duration::from_millis(300);
    assert!(!state.announce_tool_view(), "no change, no publish");
    assert!(
        live.frame().is_none(),
        "a repeated announce with no change sends nothing"
    );

    closed
        .stream
        .shutdown(std::net::Shutdown::Both)
        .expect("the client closes the stream");
    drop(closed);
    live.timeout = Duration::from_secs(5);
    assert!(change_view(state, false), "the second view change");
    let frame = live
        .frame()
        .expect("the live stream still receives changes");
    assert_eq!(
        data(&frame)["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!(1)
    );
    live.timeout = Duration::from_millis(300);
    assert!(
        live.frame().is_none(),
        "the live stream receives exactly one frame for the second change"
    );
}
