// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for minimal HTTP/1.1 wire framing shared by the
//! sync transport: request-line parsing, header lookup, and content-length
//! handling for the tiny JSON payloads the protocol uses, plus the per-request
//! resource bounds applied while reading: a 64 KiB head cap (`HEAD_CAP_BYTES`),
//! a 16 MiB body cap (`BODY_CAP_BYTES`), a 5 s per-read slice (`READ_SLICE`),
//! and the total request deadline the caller supplies.
//!
//! ## Owns
//! - Request framing helpers (`read_request`, route parse, header lookup) and
//!   the JSON POST exchange (`post_json` / `post_json_status`)
//! - The per-request read bounds: head cap, body cap, per-read slice, and
//!   in-loop deadline enforcement (values named in the constants below)
//!
//! ## Does not own
//! - Route semantics, auth, or apply behavior (`server`)
//! - Connection lifecycle: pool slots, refusal status mapping, accept loop
//!   (`server_conn`)
//! - Cursor or envelope formats (`protocol`)
//!
//! ## Boundary
//!
//! Changes to HTTP framing or to how the read bounds are enforced in the read
//! loop should normally require changes here. Changes to which status a bound
//! refusal maps to should not require changes here (`server_conn` owns that).
//!
//! Keep this module focused on HTTP wire framing and its read bounds.

use clio_types::{AmError, ErrorCode};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Largest request header block the transport will buffer: a client controls
/// the header block independently of the declared body, so without this cap a
/// client could stream an unbounded header block one small chunk at a time and
/// never declare a large body. The cap is judged on the *completed* head, so a
/// head whose terminator lands at or before this many bytes is served, and only
/// a head still unterminated at the cap is refused with 413. The buffer can
/// therefore exceed the cap by at most one read.
pub(crate) const HEAD_CAP_BYTES: usize = 64 * 1024;

/// Largest request body the sync transport will accept, checked on the
/// *declared* `Content-Length` before any body byte is read. A request that
/// declares no length is bounded by the head cap plus one read, because the
/// read stops at the terminator. Bounds one request's memory cost on this side.
/// Refused with 413.
pub(crate) const BODY_CAP_BYTES: usize = 16 * 1024 * 1024;

/// Per-read timeout slice. This is per-read only: it is renewed by every byte
/// the client sends, so a slow-dripping client never trips it. The total
/// request deadline passed to `read_request` is the real bound.
pub(crate) const READ_SLICE: Duration = Duration::from_secs(5);

/// Why `read_request` did not produce a request. The connection owner maps
/// these to refusals: oversize head/body to 413, deadline to 408, and plain
/// I/O errors to a silent close (as before the bounds existed).
#[derive(Debug)]
pub(crate) enum ReadRequestError {
    /// The header block exceeded `HEAD_CAP_BYTES`: either no terminator
    /// arrived by the cap, or the terminator sits past it.
    OversizeHead,
    /// The declared `Content-Length` exceeded `BODY_CAP_BYTES`.
    OversizeBody,
    /// The total request deadline expired before the request was complete.
    Deadline,
    /// Other I/O failure; the connection closes without a response.
    Io(std::io::Error),
}

/// One parsed HTTP request line + headers + body.
pub(crate) struct Request {
    /// Route parsed from the request path (`None` = unknown route).
    pub route: Option<Route>,
    /// `Authorization` header value, when present.
    pub authorization: Option<String>,
    /// Request body (JSON payload).
    pub body: String,
}

/// Routes exposed by the sync server transport.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Route {
    /// `POST /sync/push` — client push batch.
    Push,
    /// `POST /sync/pull` — client pull batch.
    Pull,
    /// `GET /sync/status` — server status surface.
    Status,
}

/// Shared error mappers for the transport, one per failure seam. They take
/// the driver's error by value because `map_err` passes it by value.
mod io_mappers {
    #![allow(clippy::needless_pass_by_value)]

    use super::{AmError, ErrorCode};

    pub(crate) fn read(e: std::io::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("read failed: {e}"))
    }

    pub(super) fn connect(e: std::io::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("connect failed: {e}"))
    }

    pub(super) fn timeout(e: std::io::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("timeout setup failed: {e}"))
    }

    pub(super) fn send(e: std::io::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("send failed: {e}"))
    }

    pub(super) fn recv(e: std::io::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("recv failed: {e}"))
    }

    pub(super) fn bad_response(e: serde_json::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("bad response: {e}"))
    }

    pub(super) fn bad_shape(e: serde_json::Error) -> AmError {
        AmError::new(ErrorCode::Internal, format!("bad response shape: {e}"))
    }
}

pub(crate) use io_mappers::read as io_read;
use io_mappers::{bad_response as io_bad_response, bad_shape as io_bad_shape};
use io_mappers::{connect as io_connect, recv as io_recv};
use io_mappers::{send as io_send, timeout as io_timeout};

/// Read one HTTP request off the stream under the caller's total request
/// deadline (measured from the first accepted byte toward the response).
///
/// Bounds enforced inside the loop (never only after it): the header block is
/// refused at `HEAD_CAP_BYTES`, judged on the completed head rather than before
/// the buffer is extended, so a head that ends inside the cap is served; the
/// declared `Content-Length` is refused at `BODY_CAP_BYTES` before any body
/// byte is read; each read waits at most the remaining deadline capped by
/// `READ_SLICE`, and a deadline that expires mid-read surfaces as
/// [`ReadRequestError::Deadline`].
///
/// # Errors
/// [`ReadRequestError`] for every bound and I/O failure.
pub(crate) fn read_request(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<Request, ReadRequestError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut head_end = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ReadRequestError::Deadline);
        }
        // Per-read slice, renewed by every byte; the deadline above is the
        // real bound. Setup failures are propagated, never discarded.
        stream
            .set_read_timeout(Some(remaining.min(READ_SLICE)))
            .map_err(ReadRequestError::Io)?;
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            // The slice expired without a byte; the loop re-checks the
            // remaining deadline and reports `Deadline` when it is spent.
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(ReadRequestError::Io(e)),
        };
        if n == 0 {
            break;
        }
        // The chunk is appended before the head is judged, because the
        // terminator can sit inside the very read that crosses the cap: a head
        // that ends at or before the cap must be served, and only a head with
        // no terminator by then is refused. The buffer therefore overshoots the
        // cap by at most one read, which is bounded, and a client still cannot
        // grow an unterminated header block past it.
        buf.extend_from_slice(&chunk[..n]);
        if head_end.is_none() {
            head_end = find_head_end(&buf);
        }
        match head_end {
            None => {
                // No terminator anywhere inside the cap: the next read would
                // carry the header block past the bound, so refuse before
                // accepting it.
                if buf.len() >= HEAD_CAP_BYTES {
                    return Err(ReadRequestError::OversizeHead);
                }
            }
            Some(pos) => {
                if pos > HEAD_CAP_BYTES {
                    return Err(ReadRequestError::OversizeHead);
                }
                let head = String::from_utf8_lossy(&buf[..pos]);
                let declared = content_length(&head);
                // The declared length is checked before any further body byte
                // is read, so an over-cap declaration costs nothing. With a
                // declared length this loop breaks on the full body, so the
                // declared check is the whole body bound; with no declared
                // length the read stops at the terminator, which leaves at most
                // one read of undeclared body in the buffer.
                if declared.is_some_and(|len| len > BODY_CAP_BYTES) {
                    return Err(ReadRequestError::OversizeBody);
                }
                if buf.len() >= pos + 4 + declared.unwrap_or(0) {
                    break;
                }
            }
        }
    }
    let pos = head_end.unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
    let body = String::from_utf8_lossy(&buf[pos + 4.min(buf.len())..]).to_string();
    Ok(Request {
        route: parse_route(&head),
        authorization: header_value(&head, "authorization"),
        body,
    })
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub(crate) fn parse_route(head: &str) -> Option<Route> {
    let first_line = head.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    match path {
        "/sync/push" => Some(Route::Push),
        "/sync/pull" => Some(Route::Pull),
        "/sync/status" => Some(Route::Status),
        _ => None,
    }
}

pub(crate) fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines()
        .find(|line| {
            line.split_once(':')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        })
        .and_then(|line| line.split_once(':'))
        .map(|(_, v)| v.trim().to_owned())
}

pub(crate) fn content_length(head: &str) -> Option<usize> {
    header_value(head, "content-length")?.parse().ok()
}
/// POST one JSON request to the remote sync server and decode the typed
/// response. Returns `Err` on any non-200 status (never silent refusals).
///
/// # Errors
/// Connect, transport, or decode failures; non-200 remote status.
pub(crate) fn post_json<T: serde::de::DeserializeOwned>(
    remote: &str,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> Result<T, AmError> {
    let (status, value) = post_json_status(remote, path, token, body)?;
    if status != 200 {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!("remote rejected ({status}): {value}"),
        ));
    }
    serde_json::from_value(value).map_err(io_bad_shape)
}

/// POST one JSON request and return the peer's status with its parsed JSON
/// body: the shared exchange behind [`post_json`], for a caller that must
/// keep a 4xx refusal, a retryable non-4xx answer, and a transport failure
/// distinguishable instead of collapsing them.
///
/// # Errors
/// Connect, transport, or decode failures.
pub(crate) fn post_json_status(
    remote: &str,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> Result<(u16, serde_json::Value), AmError> {
    // Serializing a `serde_json::Value` cannot fail.
    let payload = serde_json::to_vec(body).expect("serialize request");
    let mut stream = TcpStream::connect(remote).map_err(io_connect)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(io_timeout)?;
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {remote}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&payload))
        .map_err(io_send)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(io_recv)?;
    let text = String::from_utf8_lossy(&response);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = text.find("\r\n\r\n").map_or(text.len(), |p| p + 4);
    let value: serde_json::Value =
        serde_json::from_str(text[body_start..].trim()).map_err(io_bad_response)?;
    Ok((status, value))
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;
