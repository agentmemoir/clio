// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading one HTTP/1.1 request off a
//! Streamable HTTP connection under the transport resource bounds: the 64 KiB
//! head cap, the 10 MiB body cap, and the total request deadline.
//!
//! ## Owns
//! - Byte-level request reading and header parsing for the HTTP transport
//! - Deadline-driven read timeouts (408) and oversize refusals (413)
//!
//! ## Does not own
//! - Routing, auth, sessions, and response writing (see `http`)
//! - The bound values themselves (defined and documented in `http`)
//!
//! ## Boundary
//!
//! Changes to how a request is read or bounded should normally require
//! changes here. Changes to routing or tool behavior should not require
//! changes here.

use crate::http::{MAX_BODY, MAX_HEAD_BYTES};
use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::net::TcpStream;
use std::time::Instant;

/// Longest single blocking read wait. The total deadline still governs: the
/// wait never exceeds the time left before the deadline.
const READ_SLICE: std::time::Duration = std::time::Duration::from_secs(5);

/// Refusal message for the head cap, naming the bound and its value.
fn head_cap_message() -> String {
    format!(
        "header block exceeds the {} KiB head cap",
        MAX_HEAD_BYTES / 1024
    )
}

/// Refusal message for the body cap, naming the bound and its value.
fn body_cap_message() -> String {
    format!(
        "request body exceeds the {} MiB body cap",
        MAX_BODY / (1024 * 1024)
    )
}

/// One parsed HTTP request.
pub(crate) struct HttpRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

impl HttpRequest {
    #[must_use]
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(std::string::String::as_str)
    }
}

/// Read one request under the total deadline.
///
/// The error carries the refusal status and a message naming the bound and
/// its value; an empty message means a plain bad request (unchanged
/// behavior). `Ok(None)` is a clean EOF before a request (no response).
///
/// # Errors
/// Deadline expiry (408), an oversize head or body (413), or an I/O failure
/// on the read path (400).
pub(crate) fn read_request(
    stream: &mut TcpStream,
    deadline: Instant,
    deadline_secs: u64,
) -> Result<Option<HttpRequest>, (u16, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end;
    loop {
        let n = read_bounded(stream, deadline, deadline_secs, &mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        // The chunk is appended before the head is judged, because the
        // terminator can sit inside the very read that crosses the cap: a head
        // that ends at or before the cap must be served, and only a head with no
        // terminator by then is refused. The buffer therefore overshoots the cap
        // by at most one read, which is bounded, and a client still cannot grow
        // an unterminated header block past it. The head is capped separately
        // from the body because a client controls the header block and the body
        // independently, and could otherwise grow an unbounded header block one
        // small chunk at a time without ever declaring a large body.
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            header_end = pos;
            break;
        }
        // No terminator anywhere inside the cap: the next read would carry the
        // header block past the bound, so refuse before accepting it.
        if buf.len() >= MAX_HEAD_BYTES {
            return Err((413, head_cap_message()));
        }
    }
    // A head whose content ends past the cap is refused, terminator or not.
    if header_end > MAX_HEAD_BYTES {
        return Err((413, head_cap_message()));
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_uppercase();
    let path = parts.next().unwrap_or_default().to_owned();
    if method.is_empty() || path.is_empty() {
        return Err((400, String::new()));
    }
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // The declared body is refused as soon as the head is complete, before a
    // single body byte is read.
    if buf.len() > MAX_BODY || content_length > MAX_BODY {
        return Err((413, body_cap_message()));
    }
    while buf.len() < header_end + 4 + content_length {
        let n = read_bounded(stream, deadline, deadline_secs, &mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_BODY {
            return Err((413, body_cap_message()));
        }
    }
    let body = String::from_utf8_lossy(
        &buf[header_end + 4..(header_end + 4 + content_length).min(buf.len())],
    )
    .into_owned();
    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body,
    }))
}

/// One deadline-bounded read of at most `chunk.len()` bytes.
///
/// Returns the byte count; `0` is a clean EOF. The wait never exceeds the
/// time left before the deadline; expiry is a 408 naming the deadline. A read
/// that times out while deadline time remains is retried: the total deadline
/// is the only cut point, so a scheduler stall cannot manufacture a spurious
/// 400 for a client that is still inside its budget.
fn read_bounded(
    stream: &mut TcpStream,
    deadline: Instant,
    deadline_secs: u64,
    chunk: &mut [u8],
) -> Result<usize, (u16, String)> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err((
                408,
                format!("request exceeded the {deadline_secs} s total request deadline"),
            ));
        }
        // Socket setup results are propagated into the read error path: a
        // socket that cannot take a timeout cannot be bounded.
        stream
            .set_read_timeout(Some(remaining.min(READ_SLICE)))
            .map_err(|_| (400, String::new()))?;
        match stream.read(chunk) {
            Ok(n) => return Ok(n),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Err((400, String::new())),
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
