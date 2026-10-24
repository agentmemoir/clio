// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the request-batching behavior of the TEI
//! sidecar rerank adapter: splitting a shortlist into requests that fit the
//! service batch ceiling and merging the scored parts back into one
//! index-consistent result.
//!
//! ## Owns
//! - The documents-per-request split, the per-chunk wire body, the global index
//!   offset applied to each merged entry, and the fail-closed chunk error
//!
//! ## Does not own
//! - Response parsing rules (`rerank_parse`) or the single-request wire shape
//!   and provider selection (`rerank_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::rerank::rerank_tests::indices;
use crate::rerank::{DEFAULT_RERANK_MAX_BATCH, HttpReranker, Reranker};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// Generous bound on one fake-server read or accept, so a client that sends
/// fewer requests fails the test instead of hanging it.
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);

fn docs(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("doc-{i}")).collect()
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn http_unprocessable() -> String {
    let body = "{}";
    format!(
        "HTTP/1.1 422 Unprocessable Entity\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A TEI-shaped success body for one chunk: entry `i` belongs to global
/// document `start + i`, and its score encodes that global index so a missing
/// offset shift cannot pass unnoticed.
fn chunk_body(start: usize, len: usize, reverse: bool) -> Value {
    let mut locals: Vec<usize> = (0..len).collect();
    if reverse {
        locals.reverse();
    }
    Value::Array(
        locals
            .into_iter()
            .map(|i| json!({"index": i, "score": (start + i) as f64 / 100.0}))
            .collect(),
    )
}

fn read_request_body(stream: &mut std::net::TcpStream) -> String {
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

fn content_length(buf: &[u8]) -> Option<usize> {
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
fn batch_server(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
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

fn json_body(request: &str) -> Value {
    let head_end = request
        .find("\r\n\r\n")
        .expect("request carries a header terminator");
    serde_json::from_str(&request[head_end + 4..]).expect("request body is JSON")
}

fn texts(request: &str) -> Vec<String> {
    json_body(request)["texts"]
        .as_array()
        .expect("texts array")
        .iter()
        .map(|t| t.as_str().expect("text is a string").to_owned())
        .collect()
}

#[test]
fn sixty_document_shortlist_splits_into_two_capped_requests() {
    assert_eq!(DEFAULT_RERANK_MAX_BATCH, 32);
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_ok(&chunk_body(32, 28, false)),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    client
        .rerank("needle", &docs(60), 60)
        .expect("batched rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2, "one request per batch");
    for request in &captured {
        assert!(request.starts_with("POST /rerank "), "{request}");
        assert_eq!(json_body(request)["raw_scores"], json!(false));
    }
    assert_eq!(texts(&captured[0]), docs(60)[..32].to_vec());
    assert_eq!(texts(&captured[1]), docs(60)[32..].to_vec());
}

#[test]
fn merged_result_carries_global_indices_and_their_own_scores() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_ok(&chunk_body(32, 28, false)),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(60), 60).expect("rerank");
    handle.join().expect("server join");

    assert_eq!(merged.len(), 60, "every document scored");
    assert_eq!(indices(&merged), (0..60_usize).collect::<Vec<usize>>());
    for doc in &merged {
        // The score encodes the global index, so a batch without its offset
        // shift reports the second chunk as 0..27 and fails here.
        assert_eq!(
            doc.score,
            Some(doc.index as f64 / 100.0),
            "score/index mismatch at {}",
            doc.index
        );
    }
}

#[test]
fn shortlist_within_the_cap_makes_one_request() {
    let (url, handle) = batch_server(vec![http_ok(&chunk_body(0, 30, false))]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(30), 30).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 1);
    assert_eq!(texts(&captured[0]), docs(30));
    assert_eq!(indices(&merged), (0..30_usize).collect::<Vec<usize>>());
}

#[test]
fn failing_second_chunk_returns_the_http_error_and_no_partial_result() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_unprocessable(),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    let err = client
        .rerank("needle", &docs(60), 60)
        .expect_err("second chunk fails");
    handle.join().expect("server join");

    assert!(err.message.contains("HTTP 422"), "{err}");
}

#[test]
fn merged_order_is_grouped_by_chunk_not_globally_sorted() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, true)),
        http_ok(&chunk_body(32, 28, true)),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(60), 60).expect("rerank");
    handle.join().expect("server join");

    let mut expected: Vec<usize> = (0..32_usize).rev().collect();
    expected.extend((32..60_usize).rev());
    assert_eq!(indices(&merged), expected);
}

#[test]
fn zero_max_batch_is_clamped_to_one_document_per_request() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 1, false)),
        http_ok(&chunk_body(1, 1, false)),
    ]);
    let mut client = HttpReranker::new(url, None).expect("build");
    client.set_max_batch(0);
    let merged = client.rerank("needle", &docs(2), 2).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2);
    assert_eq!(texts(&captured[0]), vec!["doc-0".to_owned()]);
    assert_eq!(texts(&captured[1]), vec!["doc-1".to_owned()]);
    assert_eq!(indices(&merged), vec![0, 1]);
}
