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
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// Generous bound on one fake-server read or accept, so a client that sends
/// fewer requests fails the test instead of hanging it.
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);

/// Read one HTTP request off `stream` up to its `Content-Length` boundary.
pub(crate) fn read_request_body(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 16_384];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
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
    let head_end = request
        .find("\r\n\r\n")
        .expect("request carries a header terminator");
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
