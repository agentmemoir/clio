// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for a non-activated candidate Streamable HTTP
//! entry point: the real listener and request reading, the candidate POST-only
//! verb guard, and routing of each POST body through the candidate revision's
//! conditional-header and metadata admission.
//!
//! ## Owns
//! - The candidate accept loop and per-connection handling
//! - The candidate route: the verb guard first, then the candidate admission
//! - The response framing the route answers with
//!
//! It mints no session identifier and requires none, so every request is
//! served as the first request of its own connection.
//!
//! ## Does not own
//! - The verb decision, the conditional headers, and the admission rules (see
//!   `method_guard`, `conditional_http`, and `stateless`)
//! - Request reading and its bounds (see `http_read`)
//! - The live route, its sessions, auth, and Origin check (see `http`)
//! - Activation: only this crate's tests compile and reach this module
//!
//! ## Boundary
//!
//! Changes to how the candidate guards compose over HTTP should normally
//! require changes here. Changes to the live transport or a tool should not.
//!
//! Keep this module focused on the candidate's HTTP composition.

use crate::http::DEFAULT_DEADLINE_SECS;
use crate::http_read::{HttpRequest, read_request};
use crate::method_guard::Response;
use crate::runtime::McpState;
use crate::stateless::admit;
use std::fmt::Write as _;
use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest single blocking write wait, mirroring the live writer's bound.
const WRITE_SLICE: Duration = Duration::from_secs(5);

/// Serve the candidate endpoint over an already-bound listener until it fails.
///
/// Each connection is handled on its own thread, as the live transport does.
/// The function never returns on its own; a test binds `127.0.0.1:0`, reads the
/// port back, and drives it from another thread.
pub(crate) fn serve_candidate(state: &Arc<McpState>, listener: &TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let state = Arc::clone(state);
        std::thread::spawn(move || {
            let _ = handle_conn(&state, &mut stream);
        });
    }
}

/// Read one request and answer it through the candidate route.
fn handle_conn(state: &McpState, stream: &mut TcpStream) -> std::io::Result<()> {
    stream.set_write_timeout(Some(WRITE_SLICE))?;
    let deadline = Instant::now() + Duration::from_secs(DEFAULT_DEADLINE_SECS);
    let req = match read_request(stream, deadline, DEFAULT_DEADLINE_SECS) {
        Ok(Some(req)) => req,
        Ok(None) => return Ok(()),
        Err((status, message)) => return write_error(stream, status, &message),
    };
    let (status, body, extra) = route(state, &req);
    write_response(stream, status, &body, &extra)
}

/// The candidate route: refuse every verb but `POST`, then admit the body.
///
/// The verb guard runs before any body parser. An admitted request answers its
/// JSON-RPC envelope; a notification answers 202 with no body; a refused
/// admission answers its status and JSON-RPC error body. No session identifier
/// is read, required, or minted.
fn route(state: &McpState, req: &HttpRequest) -> Response {
    crate::method_guard::endpoint(&req.method, || {
        match admit(state, |header| req.get(header), req.body.trim()) {
            Ok(Some(response)) => (
                200,
                serde_json::to_string(&response).unwrap_or_default(),
                Vec::new(),
            ),
            Ok(None) => (202, String::new(), Vec::new()),
            Err((status, body)) => (status, body, Vec::new()),
        }
    })
}

/// Write one response: the status line, extra headers, the JSON content
/// headers, and the body.
///
/// This mirrors the live writer's framing, which stays private to `http`
/// because that file is at its line cap. The candidate tests read the bytes
/// back through the live request parser, so a framing difference fails a test.
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    extra: &[(String, String)],
) -> std::io::Result<()> {
    let mut response = String::new();
    let _ = write!(response, "HTTP/1.1 {status}\r\n");
    for (name, value) in extra {
        let _ = write!(response, "{name}: {value}\r\n");
    }
    let _ = write!(
        response,
        "Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()?;
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

/// Answer a read failure with the live transport's error body shape.
fn write_error(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let body = if message.is_empty() {
        String::new()
    } else {
        format!("{{\"error\":\"{message}\"}}")
    };
    write_response(stream, status, &body, &[])
}

#[cfg(test)]
#[path = "http_candidate_tests.rs"]
mod http_candidate_tests;

#[cfg(test)]
#[path = "http_candidate_guard_tests.rs"]
mod http_candidate_guard_tests;
