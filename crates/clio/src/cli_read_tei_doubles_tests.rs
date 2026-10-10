// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module provides the local HTTP doubles that let the CLI recall tests
//! drive a populated `semantic` arm (an embedder sidecar) and a populated
//! `reranker` arm (a rerank sidecar) without a real model server.
//!
//! ## Owns
//! - The bounded `POST /embed` and `POST /rerank` accept loops and their request caps
//! - The `McpState` factories that attach a fake embedder and/or reranker
//!
//! ## Does not own
//! - The score-object assertions (see `cli_read_scores_tests`)
//! - Score computation or the dense transform (`clio-retrieve` stage scoring)
//!
//! ## Boundary
//!
//! Changes to how a sidecar is faked should normally require changes here.
//! Changes to the score-object contract should not require changes here.
//!
//! Keep this module focused on the fake sidecars. It is test-only and never
//! compiled into production builds.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Vector size for the fake sidecar; matches the model named in the embed block.
const DENSE_DIMS: usize = 384;

/// Hard cap on served embed requests, so a stuck client can never pin the loop.
const MAX_EMBED_REQUESTS: usize = 16;

/// Hard cap on served rerank requests, so a stuck client can never pin the loop.
const MAX_RERANK_REQUESTS: usize = 16;

/// Read one HTTP request; `Content-Length` bounds the body, because the JSON
/// payload is what the rerank answer depends on and one read may be headers only.
fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            return (String::new(), Vec::new());
        }
        raw.extend_from_slice(&chunk[..read]);
        let Some(end) = raw
            .windows(4)
            .position(|window| window.starts_with(b"\r\n\r\n"))
        else {
            continue;
        };
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let length = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok());
        if raw.len() >= end + 4 + length.unwrap_or(0) {
            let request_line = head.lines().next().unwrap_or_default().to_owned();
            return (request_line, raw[end + 4..].to_vec());
        }
    }
}

fn http_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn unit_vector() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DENSE_DIMS];
    vector[0] = 1.0;
    vector
}

/// A local TEI double: a bounded accept loop answering `POST /embed` with one
/// fixed unit vector.
pub(crate) struct FakeTei {
    pub(crate) url: String,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) handle: JoinHandle<usize>,
}

/// Start the double: a non-blocking accept loop bounded by `stop` and
/// `MAX_EMBED_REQUESTS`, with a read timeout on every accepted stream.
pub(crate) fn fake_tei_server() -> FakeTei {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake TEI");
    let port = listener.local_addr().expect("fake TEI addr").port();
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let mut embed_requests = 0usize;
        while !stop_thread.load(Ordering::Relaxed) && embed_requests < MAX_EMBED_REQUESTS {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking stream");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("stream read timeout");
                    let mut request = [0_u8; 16 * 1024];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let request = &request[..read];
                    let request_line = request
                        .split(|byte| *byte == b'\r')
                        .next()
                        .unwrap_or(request);
                    let body = if request_line.starts_with(b"POST /embed") {
                        embed_requests += 1;
                        json!([unit_vector()]).to_string()
                    } else {
                        "{}".to_owned()
                    };
                    let _ = stream.write_all(http_response(&body).as_bytes());
                    let _ = stream.flush();
                }
                Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
        embed_requests
    });
    FakeTei {
        url: format!("http://127.0.0.1:{port}"),
        stop,
        handle,
    }
}

/// A local TEI double for `POST /rerank`: a bounded accept loop answering with a
/// reversed best-first permutation and recording the texts it received.
pub(crate) struct FakeRerank {
    pub(crate) url: String,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) texts: Arc<Mutex<Vec<String>>>,
    pub(crate) handle: JoinHandle<usize>,
}

/// Answer a rerank request best-first by reversing the received shortlist: the
/// entry at `rank` carries document index `count - 1 - rank` with score
/// `0.9 - 0.1 * rank`, so each hit's value is exactly predictable.
fn rerank_body(body: &[u8], texts: &Mutex<Vec<String>>) -> String {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let docs: Vec<String> = request["texts"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|text| text.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut ranked = Vec::with_capacity(docs.len());
    let mut score = 0.9_f64;
    for rank in 0..docs.len() {
        ranked.push(json!({"index": docs.len() - 1 - rank, "score": score}));
        score -= 0.1;
    }
    *texts.lock().expect("record rerank texts") = docs;
    json!(ranked).to_string()
}

/// Start the rerank double: a non-blocking accept loop bounded by `stop` and
/// `MAX_RERANK_REQUESTS`, with a read timeout on every accepted stream.
pub(crate) fn fake_rerank_server() -> FakeRerank {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rerank");
    let port = listener.local_addr().expect("fake rerank addr").port();
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let texts = Arc::new(Mutex::new(Vec::new()));
    let texts_thread = Arc::clone(&texts);
    let handle = thread::spawn(move || {
        let mut rerank_requests = 0usize;
        while !stop_thread.load(Ordering::Relaxed) && rerank_requests < MAX_RERANK_REQUESTS {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking stream");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("stream read timeout");
                    let (request_line, body) = read_request(&mut stream);
                    let body = if request_line.starts_with("POST /rerank") {
                        rerank_requests += 1;
                        rerank_body(&body, &texts_thread)
                    } else {
                        "{}".to_owned()
                    };
                    let _ = stream.write_all(http_response(&body).as_bytes());
                    let _ = stream.flush();
                }
                Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
        rerank_requests
    });
    FakeRerank {
        url: format!("http://127.0.0.1:{port}"),
        stop,
        texts,
        handle,
    }
}

fn embed_block(url: &str) -> Value {
    json!({
        "url": url,
        "model": "BAAI/bge-small-en-v1.5",
        "dims": DENSE_DIMS,
        "provider": "tei"
    })
}

fn rerank_block(url: &str) -> Value {
    json!({
        "url": url,
        "model": "fake-rerank-tei",
        "provider": "tei"
    })
}

fn open_with_blocks(effective: &Value) -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), effective).expect("open state")
}

/// Open in-memory state whose effective config attaches the fake embedder, so
/// the dense arm is live for both the index drain and retrieval.
pub(crate) fn state_with_live_embedder(url: &str) -> Arc<McpState> {
    open_with_blocks(&json!({ "embed": embed_block(url) }))
}

/// Open in-memory state whose effective config attaches the fake reranker, so
/// the optional second stage is live for CLI recall.
pub(crate) fn state_with_reranker(url: &str) -> Arc<McpState> {
    open_with_blocks(&json!({ "rerank": rerank_block(url) }))
}

/// Open in-memory state with BOTH fakes attached, so every arm of the score
/// breakdown is populated on the same hit.
pub(crate) fn state_with_embedder_and_reranker(embed_url: &str, rerank_url: &str) -> Arc<McpState> {
    open_with_blocks(&json!({
        "embed": embed_block(embed_url),
        "rerank": rerank_block(rerank_url)
    }))
}
