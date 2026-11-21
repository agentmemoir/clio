// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test-only fake rerank service and HTTP
//! request decoder that the rerank batch tests and the end-to-end retrieval
//! rerank tests both drive.
//!
//! ## Owns
//! - `batch_server`: a multi-connection fake `/rerank` service that answers one
//!   response per accepted connection and returns every captured request
//! - The request decoder helpers (`read_request_body`, `content_length`,
//!   `json_body`, `texts`) those tests assert on
//!
//! ## Does not own
//! - Any test case or assertion (`rerank_batch_tests`, `hybrid_rerank_tests`)
//! - The single-connection capture server the adapter tests use
//!   (`fixtures_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use serde_json::Value;
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// Generous bound on one fake-server read or accept, so a client that sends
/// fewer requests fails the test instead of hanging it.
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);

/// Read one HTTP request off `reader` up to its `Content-Length` boundary.
///
/// Generic over `Read` so a test can drive it with an in-memory reader that
/// injects an interrupted read without a socket.
pub(crate) fn read_request_body(reader: &mut impl Read) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 16_384];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                require_header_terminator(&buf, std::io::ErrorKind::UnexpectedEof);
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            // An interrupted syscall is not a fault: retry it.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                require_header_terminator(&buf, e.kind());
                break;
            }
        }
        if let Some(len) = content_length(&buf) {
            let head = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("headers");
            if buf.len() >= head + 4 + len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Fail with the byte count and the error kind when a read ended before the
/// header block. A read that ends after the terminator is a normal end.
fn require_header_terminator(buf: &[u8], kind: std::io::ErrorKind) {
    assert!(
        buf.windows(4).any(|w| w == b"\r\n\r\n"),
        "fake server read ended after {} bytes before the header block: {kind:?}",
        buf.len()
    );
}

/// The `Content-Length` of a partially received request, once its header
/// block is complete.
pub(crate) fn content_length(buf: &[u8]) -> Option<usize> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
}

/// Fake rerank service answering `responses` in order, one connection each.
///
/// Captures every raw request and returns them from the join handle, so a test
/// can assert how many requests were sent and what each carried. The accept
/// loop is bounded by `SERVER_TIMEOUT`: the deadline is a safety net, not the
/// synchronization, which is the accepted connection itself.
pub(crate) fn batch_server(
    responses: Vec<String>,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake batch server");
    let addr = listener.local_addr().expect("addr");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + SERVER_TIMEOUT;
        let mut captured = Vec::with_capacity(responses.len());
        for response in responses {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(accepted) => break accepted,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "fake server waited {SERVER_TIMEOUT:?} for a connection that never came"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("accept failed: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(SERVER_TIMEOUT))
                .expect("read timeout");
            captured.push(read_request_body(&mut stream));
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.flush().expect("flush response");
        }
        captured
    });
    (format!("http://127.0.0.1:{}", addr.port()), handle)
}

/// The JSON body of a captured request.
pub(crate) fn json_body(request: &str) -> Value {
    let Some(head_end) = request.find("\r\n\r\n") else {
        let head: String = request.chars().take(80).collect();
        panic!(
            "request is {} bytes and has no header terminator; head: {head:?}",
            request.len()
        );
    };
    serde_json::from_str(&request[head_end + 4..]).expect("request body is JSON")
}

/// The `texts` array of a captured rerank request.
pub(crate) fn texts(request: &str) -> Vec<String> {
    json_body(request)["texts"]
        .as_array()
        .expect("texts array")
        .iter()
        .map(|t| t.as_str().expect("text is a string").to_owned())
        .collect()
}

/// A reader that reports one interrupted read, then serves `bytes` normally.
struct InterruptOnce {
    bytes: Cursor<Vec<u8>>,
    interrupted: bool,
}

impl Read for InterruptOnce {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        self.bytes.read(buf)
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        String::new()
    }
}

#[test]
fn an_interrupted_read_is_retried_and_the_full_request_is_captured() {
    let body = r#"{"texts":["a"]}"#;
    let request = format!(
        "POST /rerank HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut reader = InterruptOnce {
        bytes: Cursor::new(request.into_bytes()),
        interrupted: false,
    };

    let captured = read_request_body(&mut reader);

    assert!(
        captured.contains("\r\n\r\n"),
        "captured request lacks a header terminator: {captured:?}"
    );
    assert_eq!(json_body(&captured)["texts"], serde_json::json!(["a"]));
}

#[test]
fn a_read_that_ends_before_the_header_block_reports_the_byte_count_and_kind() {
    let partial = b"POST /rerank HTTP/1.1\r\ncontent-length: 2\r\n";
    let mut reader = &partial[..];
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_request_body(&mut reader)
    }))
    .expect_err("a read that ends before the header block must panic");

    let message = panic_text(&*payload);
    assert!(
        message.contains(&format!(
            "read ended after {} bytes before the header block",
            partial.len()
        )),
        "{message}"
    );
    assert!(message.contains("UnexpectedEof"), "{message}");
}

#[test]
fn batch_server_answers_and_returns_the_request_it_captured() {
    let response = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
    let (url, handle) = batch_server(vec![response.to_owned()]);
    let body = r#"{"texts":[]}"#;
    let request = format!(
        "POST /rerank HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream =
        std::net::TcpStream::connect(url.trim_start_matches("http://")).expect("connect");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read response");

    assert_eq!(reply, response);
    let captured = handle.join().expect("server thread");
    assert_eq!(texts(&captured[0]), Vec::<String>::new());
}

#[test]
fn json_body_names_the_capture_when_the_header_terminator_is_missing() {
    let request = "GET /rerank HTTP/1.1\r\nhost: localhost\r\n";
    let payload = std::panic::catch_unwind(|| json_body(request))
        .expect_err("a request with no header terminator must panic");

    let message = panic_text(&*payload);
    assert!(
        message.contains(&format!("request is {} bytes", request.len())),
        "{message}"
    );
    assert!(message.contains("GET /rerank HTTP/1.1"), "{message}");
}
