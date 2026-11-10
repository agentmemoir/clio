// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the write path of the Streamable HTTP
//! transport: the header block, the buffered body, and the streamed body, read
//! through a real socket by a real client.
//!
//! ## Owns
//! - The streamed answer's headers: SSE content type, no caching, no proxy
//!   buffering, a close, and no `Content-Length`
//! - The head frame and the queued frame that follow those headers
//! - The buffered answer's `Content-Length`
//!
//! ## Does not own
//! - The status line and header helpers (see `http`)
//! - The frame encoding or the write loop (see `event_stream`)
//! - Any route decision (see `http_candidate`)
//!
//! ## Boundary
//!
//! Changes to what one answer writes on the wire should normally require
//! changes here. Changes to a gate, a route, or a frame should not.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::event_stream::{EventStream, FrameSource, frame};
use crate::http::http_test_support_tests::{parse_response, post_json};
use crate::http_post::{PostBody, PostContext, PostResponse, PostRoute};
use crate::runtime::McpOpenOptions;
use std::io::Read;
use std::net::TcpStream;

/// The acknowledgement frame a streamed answer writes before its first wait.
fn head_frame() -> Vec<u8> {
    frame("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}")
}

/// The one queued frame the test source yields after the head.
fn queued_frame() -> Vec<u8> {
    frame("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}")
}

/// A source that yields one queued frame, then reports that no frame arrived.
struct OneFrame(Option<Vec<u8>>);

impl FrameSource for OneFrame {
    fn next(&mut self, _timeout: Duration) -> Option<Vec<u8>> {
        self.0.take()
    }
}

/// A route that answers with a head frame and one queued frame.
struct StreamRoute;

impl PostRoute for StreamRoute {
    fn respond(&self, _ctx: PostContext<'_>) -> PostResponse {
        (
            200,
            PostBody::Stream(EventStream {
                head: head_frame(),
                source: Box::new(OneFrame(Some(queued_frame()))),
            }),
            Vec::new(),
        )
    }
}

/// A route that answers with one fixed buffered body.
struct BufferedRoute(&'static str);

impl PostRoute for BufferedRoute {
    fn respond(&self, _ctx: PostContext<'_>) -> PostResponse {
        (200, PostBody::Buffered(self.0.to_owned()), Vec::new())
    }
}

/// Serve `strategy` through the production accept loop on an OS-assigned port
/// and return that port.
fn spawn<P: PostRoute + Send + Sync + 'static>(strategy: P) -> u16 {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let cfg = HttpConfig {
            insecure_no_auth: true,
            ..HttpConfig::default()
        };
        let _ = serve_with_listener_and(state, cfg, listener, Arc::new(strategy));
    });
    port
}

/// POST one request over a raw socket and read until `stop` arrives or the read
/// times out. A stream that keeps answering cannot hold the test past the read
/// timeout.
fn read_until(port: u16, stop: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let body = "{}";
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !String::from_utf8_lossy(&buf).contains(stop) {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
        }
    }
    buf
}

/// A real request gets the SSE headers, no `Content-Length`, the head frame,
/// and the queued frame, in that order.
#[test]
fn a_streamed_response_sends_headers_frames_and_no_content_length() {
    let port = spawn(StreamRoute);
    let stop = String::from_utf8(queued_frame()).expect("a frame is UTF-8");
    // The read stops at the queued frame's trailing blank line, so the frame is
    // whole before the assertions read it.
    let response = parse_response(&read_until(port, &stop));
    assert_eq!(response.status, 200, "{:?}", response.headers);
    assert_eq!(
        response.headers.get("content-type").map(String::as_str),
        Some("text/event-stream")
    );
    assert_eq!(
        response
            .headers
            .get("x-accel-buffering")
            .map(String::as_str),
        Some("no")
    );
    assert!(
        !response.headers.contains_key("content-length"),
        "a stream carries no length: {:?}",
        response.headers
    );
    let head = String::from_utf8(head_frame()).expect("a frame is UTF-8");
    assert!(
        response.body.starts_with(&head),
        "the head frame comes first: {:?}",
        response.body
    );
    assert!(
        response.body.contains(&stop),
        "the queued frame follows the head: {:?}",
        response.body
    );
}

/// A buffered answer still declares the exact length of its body.
#[test]
fn a_buffered_response_still_carries_content_length() {
    let port = spawn(BufferedRoute("{\"ok\":true}"));
    let response = post_json(port, "{}", "");
    assert_eq!(response.status, 200, "{:?}", response.headers);
    let length: usize = response
        .headers
        .get("content-length")
        .expect("a buffered answer carries a length")
        .parse()
        .expect("a numeric length");
    assert_eq!(length, response.body.len(), "{:?}", response.body);
    assert_eq!(response.body, "{\"ok\":true}");
}
