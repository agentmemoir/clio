// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Streamable HTTP MCP transport per the
//! pinned `2025-11-25` revision: one JSON-RPC endpoint, optional
//! `MCP-Session-Id` lifecycle, Origin validation, and token auth.
//!
//! ## Owns
//! - Minimal HTTP/1.1 request parsing and response writing for `/mcp`
//! - The accept loop, connection cap, and per-request dispatch through the
//!   bound protocol entry point
//! - The credential source this server accepts (the operator's token store) and
//!   the loopback rule that gates it
//! - The transport resource bounds (values defined below):
//!   - 64 KiB head cap (`MAX_HEAD_BYTES`) and 10 MiB body cap (`MAX_BODY`),
//!     capped separately because a client controls the header block and the
//!     body independently
//!   - 30 s total request deadline, bounded to *reading* the request:
//!     first accepted byte through the end of `read_request`, overridable per
//!     deployment via `HttpConfig::deadline_secs` (`DEFAULT_DEADLINE_SECS`).
//!     Routing and the response write run after the last check, so a slow route
//!     is not cut off — writes are bounded separately by `WRITE_SLICE`
//!   - 32 concurrent-connection cap (`conn_pool::MAX_CONNECTIONS`); above the
//!     cap the accept loop answers 503 and closes
//!
//! ## Does not own
//! - Protocol handling (see `protocol`)
//! - Legacy HTTP+SSE (optional compatibility, not implemented as primary)
//! - Request byte reading under those bounds (see `http_read`)
//! - Origin validation and session lifecycle (see `http_guard`)
//! - Reading the credential store and resolving a presented secret (see
//!   `http_auth`)
//! - Slot counting and release (see `conn_pool`)
//!
//! ## Boundary
//!
//! Changes to HTTP transport security or session handling should normally
//! require changes here or in `http_guard`. Changes to tool behavior should not
//! require changes here.
//!
//! Keep this module focused on the Streamable HTTP transport.

use crate::conn_pool::{ConnPool, MAX_CONNECTIONS};
use crate::http_auth::{AuthOutcome, CredentialStore, authorize_request};
use crate::http_guard::{HttpShared, check_origin, handle_delete, mint_session};
use crate::http_read::{HttpRequest, read_request};
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use serde_json::Value;
use std::fmt::Write as _;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Maximum accepted request body, 10 MiB. Guards unbounded reads: a client
/// controls the header block and the body independently, and without this cap
/// it could push unbounded body bytes one small chunk at a time. The value is
/// recorded and pinned by a test; a reviewer who finds a raised `MAX_BODY`
/// has found a defect.
pub(crate) const MAX_BODY: usize = 10 * 1024 * 1024;

/// Maximum accepted header block, 64 KiB. Guards unbounded header growth: a
/// client can grow the head one small chunk at a time without ever declaring
/// a large body, so the head is capped separately from `MAX_BODY`.
pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Total request deadline, applied when `HttpConfig::deadline_secs` is `None`.
/// A per-read timeout alone does not bound a client that drips bytes, so the
/// deadline is total — but it bounds *reading*, from the first accepted byte to
/// the end of the read. Routing and the response write run after the last
/// check; the write is bounded per call by [`WRITE_SLICE`] instead.
/// Recorded and pinned by a test.
pub(crate) const DEFAULT_DEADLINE_SECS: u64 = 30;

/// Longest single blocking write wait for response bytes, so a peer that
/// never reads cannot hold a connection slot forever.
const WRITE_SLICE: Duration = Duration::from_secs(5);

/// Write timeout for the above-cap connection refusal, kept short so a
/// hostile non-reading peer cannot stall the accept loop.
const REFUSAL_WRITE_SLICE: Duration = Duration::from_secs(2);

/// Configuration for the Streamable HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Bind address; must be loopback unless the token store holds a usable
    /// record.
    pub bind: String,
    /// Loaded network-credential store (actor plus banks per record).
    ///
    /// `Some` with an empty store authorizes nothing, which is why a bind that
    /// requires a credential treats it as no credential source at all.
    pub credentials: Option<Arc<CredentialStore>>,
    /// Mint and enforce `MCP-Session-Id` sessions.
    pub sessions_enabled: bool,
    /// Extra allowed Origin hosts (localhost is always allowed).
    pub allowed_origins: Vec<String>,
    /// Total request deadline override in seconds, bounding the *read* of one
    /// request (first accepted byte through the end of the read). `None` keeps
    /// the 30 s default (`DEFAULT_DEADLINE_SECS`). This is the deployment
    /// raise/lower knob; the default keeps a slow or silent client from holding
    /// a connection slot forever. Routing and the response write are not
    /// covered; the write is bounded per call by `WRITE_SLICE`.
    pub deadline_secs: Option<u64>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".into(),
            credentials: None,
            sessions_enabled: false,
            allowed_origins: vec![],
            deadline_secs: None,
        }
    }
}

/// Serve MCP over Streamable HTTP until the listener fails.
///
/// # Errors
/// Listener setup failures; a non-loopback bind without a token fails closed.
pub fn serve(state: Arc<McpState>, cfg: HttpConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(&cfg.bind)?;
    serve_with_listener(state, cfg, listener)
}

/// Serve MCP over an already-bound listener (testable entrypoint).
///
/// # Errors
/// Accept or per-connection I/O failures are logged and the loop continues.
#[allow(clippy::needless_pass_by_value)]
pub fn serve_with_listener(
    state: Arc<McpState>,
    cfg: HttpConfig,
    listener: TcpListener,
) -> std::io::Result<()> {
    let loopback = listener.local_addr().is_ok_and(|a| a.ip().is_loopback());
    if !loopback && !has_credential_source(&cfg) {
        // Auth is required outside dev mode (non-loopback bind), and a store
        // holding no record authorizes nothing, so it is not a source. The
        // refusal names the one way an operator satisfies the rule, so it reads
        // the same as the CLI's own refusal. The opening phrase is the pinned
        // substring an operator's runbook and a search hold.
        return Err(std::io::Error::other(
            "mcp http: non-loopback bind requires an auth token: point --token-file \
             at a store holding at least one record",
        ));
    }
    let handler = Arc::new(McpHandler::new(state));
    let shared = Arc::new(HttpShared::default());
    let pool = Arc::new(ConnPool::default());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        // `try_acquire` never blocks: a full pool refuses immediately instead
        // of queueing unbounded work behind slow clients.
        let Some(slot) = pool.try_acquire() else {
            refuse_over_cap(&mut stream);
            continue;
        };
        let handler = Arc::clone(&handler);
        let cfg = cfg.clone();
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            // The slot guard is held for the whole connection and released by
            // Drop on every exit path: success, client error, server error,
            // write failure, timeout, or panic.
            let _slot = slot;
            let _ = handle_conn(handler, cfg, shared, stream);
        });
    }
    Ok(())
}

/// Answer a connection accepted above the connection cap with a readable 503
/// and close. Best-effort by decision: a peer that never reads must not stall
/// the accept loop, so the write timeout is short and setup or write failures
/// just drop the connection.
fn refuse_over_cap(stream: &mut TcpStream) {
    let _ = stream.set_write_timeout(Some(REFUSAL_WRITE_SLICE));
    let message = format!("connection limit reached ({MAX_CONNECTIONS}); retry later");
    let _ = write_simple(stream, 503, &message);
}

#[allow(clippy::needless_pass_by_value)]
fn handle_conn(
    handler: Arc<McpHandler>,
    cfg: HttpConfig,
    shared: Arc<HttpShared>,
    mut stream: TcpStream,
) -> std::io::Result<()> {
    // Total request deadline, measured from the first accepted byte (accept
    // hands the connection over immediately) and bounding the read. A per-read
    // timeout alone does not bound a client that drips bytes. Nothing checks it
    // after `read_request` returns, so routing is not cut off.
    let deadline_secs = cfg.deadline_secs.unwrap_or(DEFAULT_DEADLINE_SECS);
    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    // Socket setup results are propagated: a socket that cannot take a
    // timeout cannot be bounded, so it must not hold a connection slot.
    stream.set_write_timeout(Some(WRITE_SLICE))?;
    let req = match read_request(&mut stream, deadline, deadline_secs) {
        Ok(Some(req)) => req,
        Ok(None) => return Ok(()),
        Err((status, message)) => {
            return write_simple(&mut stream, status, &message);
        }
    };
    let (status, body, extra) = route(&handler, &cfg, &shared, &req);
    let mut response = String::new();
    let _ = write!(response, "HTTP/1.1 {status}\r\n");
    for (k, v) in extra {
        let _ = write!(response, "{k}: {v}\r\n");
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
    finish(&mut stream);
    Ok(())
}

/// Core request routing; returns `(status, body, extra_headers)`.
#[must_use]
fn route(
    handler: &Arc<McpHandler>,
    cfg: &HttpConfig,
    shared: &HttpShared,
    req: &HttpRequest,
) -> (u16, String, Vec<(String, String)>) {
    if req.method == "GET" {
        // Server-initiated SSE stream is not offered under this write surface;
        // the pinned revision allows a 405 refusal.
        return (405, String::new(), vec![]);
    }
    if req.method != "POST" && req.method != "DELETE" {
        return (405, String::new(), vec![]);
    }
    if let Some(status) = check_origin(cfg, req) {
        return (status, String::new(), vec![]);
    }
    // Origin first, then credentials: a request from an unapproved Origin never
    // reaches the credential comparison at all.
    let principal = match authorize_request(cfg.credentials.as_deref(), req.get("authorization")) {
        Ok(outcome) => outcome,
        Err(status) => return (status, String::new(), vec![]),
    };
    // Auth already decided this request's principal; the session gate below
    // compares the presenting principal's identity, so it is read before
    // `scoped_state` consumes the outcome. The identity is not a secret.
    let identity = match &principal {
        AuthOutcome::Bound(principal) => Some(principal.session_identity()),
        AuthOutcome::Unbound => None,
    };
    if req.method == "DELETE" {
        return handle_delete(shared, req, identity.as_deref());
    }
    if req.path != "/mcp" {
        return (404, String::new(), vec![]);
    }
    let accepts = req
        .get("accept")
        .map_or_else(String::new, str::to_ascii_lowercase);
    if !accepts_json(&accepts) {
        return (406, String::new(), vec![]);
    }
    let content_type = req
        .get("content-type")
        .unwrap_or("application/json")
        .to_ascii_lowercase();
    if !content_type.starts_with("application/json") {
        return (415, String::new(), vec![]);
    }
    let parsed = serde_json::from_str::<Value>(req.body.trim()).ok();
    let request_method = parsed
        .as_ref()
        .and_then(|v| v.get("method"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if cfg.sessions_enabled && request_method != "initialize" {
        let known = req
            .get("mcp-session-id")
            .is_some_and(|id| shared.is_live(id, identity.as_deref()));
        if !known {
            return (404, String::new(), vec![]);
        }
    }
    // The request-scoped state carries the credential's principal, so dispatch
    // runs through the bound entry point and the shared `Mutex`-held state stays
    // one instance rather than a per-request fork. `Unbound` is the loopback dev
    // default: the base state, unchanged.
    let request_state = scoped_state(handler, principal);
    let Some(response) = handler.handle_frame_with_state(&request_state, req.body.trim()) else {
        // Notification-only POST: 202 with no body (pinned revision rule).
        return (202, String::new(), vec![]);
    };
    let mut extra = Vec::new();
    if cfg.sessions_enabled && request_method == "initialize" {
        let id = mint_session(shared, identity.as_deref());
        extra.push(("MCP-Session-Id".into(), id));
    }
    (
        200,
        serde_json::to_string(&response).unwrap_or_default(),
        extra,
    )
}

/// Whether this config authorizes anyone over the network.
///
/// A store is usable only when it holds a record: an empty store makes every
/// request a 401, so treating it as a source would only hide that at start.
#[must_use]
pub fn has_credential_source(cfg: &HttpConfig) -> bool {
    cfg.credentials.as_ref().is_some_and(|c| !c.is_empty())
}

/// The state one request dispatches against: a shallow clone of the base state
/// carrying this request's principal.
///
/// The clone is the point. Every field that must stay single and unforked is
/// already behind an `Arc` on `McpState`, so a per-request copy shares the store
/// engines, the index drain, the retention store, and the config runtime instead
/// of duplicating them.
fn scoped_state(handler: &Arc<McpHandler>, principal: AuthOutcome) -> McpState {
    let base = handler.state();
    match principal {
        AuthOutcome::Bound(principal) => base.with_principal(principal),
        AuthOutcome::Unbound => base.as_ref().clone(),
    }
}

fn accepts_json(accept: &str) -> bool {
    accept.is_empty()
        || accept.contains("application/json")
        || accept.contains("*/*")
        || accept.contains("text/event-stream")
}

fn write_simple(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    // An empty message means an empty body: Content-Length must match exactly,
    // otherwise strict clients read a truncated response.
    let body = if message.is_empty() {
        String::new()
    } else {
        format!("{{\"error\":\"{message}\"}}")
    };
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_text(status),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()?;
    finish(stream);
    Ok(())
}

/// Half-close the write side, so dropping the socket is a graceful FIN rather
/// than an abortive reset.
///
/// The head promised `Connection: close`, so the FIN belongs here and is queued
/// behind the payload the peer has already been promised. It matters on the
/// refusal paths, which answer without reading the whole request: a socket
/// closed with the peer's bytes still unread in its receive queue is closed
/// abortively (RFC 9293 ABORT), and the resulting RST discards the
/// response the peer has not read yet — a refusal that arrived as a truncated
/// body and a connection error. Draining instead would fix it too, but the
/// cap refusal runs in the accept loop, so waiting for a client to stop sending
/// is the accept-loop stall that waiting for a client to stop sending would
/// reintroduce. One syscall, no wait.
///
/// Best-effort by decision: a peer that has already gone makes this fail, and
/// there is nothing left to say to it.
fn finish(stream: &mut TcpStream) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

#[must_use]
fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
#[path = "http_binding_tests.rs"]
mod http_binding_tests;
#[cfg(test)]
#[path = "http_request_bounds_tests.rs"]
mod http_request_bounds_tests;
#[cfg(test)]
#[path = "http_test_support_tests.rs"]
mod http_test_support_tests;
#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;
