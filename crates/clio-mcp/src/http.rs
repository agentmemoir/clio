// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Streamable HTTP MCP transport under the
//! one stateless revision this server implements: one JSON-RPC endpoint, Origin
//! validation, and token auth.
//!
//! ## Owns
//! - Minimal HTTP/1.1 request parsing and response writing for `/mcp`
//! - The accept loop, connection cap, and per-request dispatch (`http_candidate`)
//! - The credential source this server accepts (the operator's token store); the
//!   bind rule that requires one unless explicitly opted out lives in `http_start_guard`
//! - The transport resource bounds (values defined below):
//!   - 64 KiB head cap (`MAX_HEAD_BYTES`) and 10 MiB body cap (`MAX_BODY`),
//!     separate because a client controls the header block and body apart
//!   - 30 s total request deadline for *reading* one request, from the first
//!     accepted byte to the end of `read_request`
//!     (`HttpConfig::deadline_secs`, `DEFAULT_DEADLINE_SECS`). Routing and the
//!     write run after it; the write is bounded by `WRITE_SLICE` per call
//!   - 2 s header-read slice of that total, clamped to it
//!     (`DEFAULT_HEAD_DEADLINE_SECS`, `head_deadline_secs`), so a partial
//!     unauthenticated head cannot hold a slot for the full total
//!   - 32 concurrent-connection cap (`conn_pool::MAX_CONNECTIONS`); above the
//!     cap the accept loop answers 503 and closes
//!   - A streamed answer is bounded by `WRITE_SLICE` per write and by the
//!     connection cap, not by the request deadline, which stops at the read end
//!
//! ## Does not own
//! - Protocol handling (see `protocol`)
//! - Legacy HTTP+SSE (optional compatibility, not implemented as primary)
//! - Request byte reading under those bounds (see `http_read`)
//! - Origin validation (see `http_guard`)
//! - Reading the credential store and resolving a presented secret (`http_auth`)
//! - Slot counting and release (see `conn_pool`)
//!
//! ## Boundary
//!
//! Changes to HTTP transport security or request gating should normally
//! require changes here or in `http_guard`. Changes to tool behavior should not
//! require changes here.
//!
//! Keep this module focused on the Streamable HTTP transport.

use crate::conn_pool::{ConnPool, MAX_CONNECTIONS};
use crate::http::http_start_guard::{BindRefusal, bind_refusal};
use crate::http_auth::{CredentialStore, authorize_request};
use crate::http_candidate::CandidatePost;
use crate::http_guard::check_origin;
use crate::http_post::{PostBody, PostContext, PostResponse, PostRoute, scoped_state};
use crate::http_read::{HttpRequest, ReadBound, read_request};
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use std::fmt::Write as _;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "http_start_guard.rs"]
pub mod http_start_guard;

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

/// Header-read deadline, applied when `HttpConfig::head_deadline_secs` is
/// `None` and clamped to the total deadline. Pinned by a test.
pub(crate) const DEFAULT_HEAD_DEADLINE_SECS: u64 = 2;

/// Longest single blocking write wait for response bytes, so a peer that
/// never reads cannot hold a connection slot forever.
const WRITE_SLICE: Duration = Duration::from_secs(5);

/// Write timeout for the above-cap connection refusal, kept short so a
/// hostile non-reading peer cannot stall the accept loop.
const REFUSAL_WRITE_SLICE: Duration = Duration::from_secs(2);

/// The header block a streamed answer sends after the status and extra headers:
/// the SSE content type, no caching, no proxy buffering, and a close. A stream
/// carries no `Content-Length`, because its length is unknown when the head is
/// written.
const STREAM_HEADERS: &str = "Content-Type: text/event-stream\r\nCache-Control: no-store\r\n\
     X-Accel-Buffering: no\r\nConnection: close\r\n\r\n";

/// Configuration for the Streamable HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Bind address; serving it requires a credential source unless
    /// `insecure_no_auth` is set.
    pub bind: String,
    /// Loaded network-credential store (actor plus banks per record).
    ///
    /// `Some` with an empty store authorizes nothing, which is why a bind that
    /// requires a credential treats it as no credential source at all.
    pub credentials: Option<Arc<CredentialStore>>,
    /// Serve without requiring any credential, even on loopback.
    ///
    /// Off by default: every bind requires a credential store unless this is
    /// set. It is the explicit operator opt-out behind `--insecure-no-auth`; a
    /// loopback bind with no `--token-file` is otherwise refused at start.
    pub insecure_no_auth: bool,
    /// Also allow a non-loopback bind under `insecure_no_auth`; off by default.
    pub insecure_bind_any: bool,
    /// Extra allowed Origin hosts (localhost is always allowed).
    pub allowed_origins: Vec<String>,
    /// Total request deadline override in seconds, bounding the *read* of one
    /// request (first accepted byte through the end of the read). `None` keeps
    /// the 30 s default (`DEFAULT_DEADLINE_SECS`); the default keeps a slow or
    /// silent client from holding a slot forever. Routing and the response
    /// write are not covered; the write is bounded by `WRITE_SLICE`.
    pub deadline_secs: Option<u64>,
    /// Header-read deadline override in seconds, bounding the head read until
    /// the blank line. `None` keeps `DEFAULT_HEAD_DEADLINE_SECS` (2 s). The
    /// value has a 1 s floor and is clamped to the total deadline.
    pub head_deadline_secs: Option<u64>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".into(),
            credentials: None,
            insecure_no_auth: false,
            insecure_bind_any: false,
            allowed_origins: vec![],
            deadline_secs: None,
            head_deadline_secs: None,
        }
    }
}

/// Serve MCP over Streamable HTTP until the listener fails.
///
/// # Errors
/// Listener setup failures; a bind without a credential fails closed unless the
/// operator opted out (both opt-outs outside loopback).
pub fn serve(state: Arc<McpState>, cfg: HttpConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(&cfg.bind)?;
    serve_with_listener(state, cfg, listener)
}

/// Serve MCP over an already-bound listener (testable entrypoint).
#[allow(clippy::needless_pass_by_value)]
pub fn serve_with_listener(
    state: Arc<McpState>,
    cfg: HttpConfig,
    listener: TcpListener,
) -> std::io::Result<()> {
    serve_with_listener_and(state, cfg, listener, Arc::new(CandidatePost))
}

/// Serve over an already-bound listener with the given POST dispatch strategy.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn serve_with_listener_and<P>(
    state: Arc<McpState>,
    cfg: HttpConfig,
    listener: TcpListener,
    post: Arc<P>,
) -> std::io::Result<()>
where
    P: PostRoute + Send + Sync + 'static,
{
    let loopback = listener.local_addr().is_ok_and(|a| a.ip().is_loopback());
    if let Some(refusal) = bind_refusal(&cfg, loopback) {
        // A store with no record authorizes nothing, so it is not a source. Each
        // refusal names the one way an operator satisfies the rule, and the
        // opening phrases are pinned substrings a runbook and a search hold.
        let message = match refusal {
            BindRefusal::Loopback => {
                "mcp http: loopback bind requires an auth token: point --token-file at a store \
                 holding at least one record, or pass --insecure-no-auth to serve without \
                 authentication"
            }
            BindRefusal::NonLoopback => {
                "mcp http: non-loopback bind requires an auth token: point --token-file at a \
                 store holding at least one record, or pass --insecure-no-auth with \
                 --insecure-bind-any to serve without authentication"
            }
        };
        return Err(std::io::Error::other(message));
    }
    let handler = Arc::new(McpHandler::new(state));
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
        let post = Arc::clone(&post);
        std::thread::spawn(move || {
            // The slot guard is held for the whole connection and released by
            // Drop on every exit path: success, client error, server error,
            // write failure, timeout, or panic.
            let _slot = slot;
            let _ = handle_conn(handler, cfg, post, stream);
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
fn handle_conn<P>(
    handler: Arc<McpHandler>,
    cfg: HttpConfig,
    post: Arc<P>,
    mut stream: TcpStream,
) -> std::io::Result<()>
where
    P: PostRoute,
{
    // Total request deadline, from the first accepted byte (accept hands the
    // connection over immediately) and bounding the read; the header budget is a
    // slice of this same clock. A per-read timeout alone cannot bound a drip, and
    // routing is not cut off because nothing checks it after the read returns.
    let now = Instant::now();
    let deadline_secs = cfg.deadline_secs.unwrap_or(DEFAULT_DEADLINE_SECS);
    let total = ReadBound::total(now, deadline_secs);
    let head_secs = cfg.head_deadline_secs.unwrap_or(DEFAULT_HEAD_DEADLINE_SECS);
    let head = ReadBound::head(now, deadline_secs, head_secs);
    // Socket setup results are propagated: a socket that cannot take a
    // timeout cannot be bounded, so it must not hold a connection slot.
    stream.set_write_timeout(Some(WRITE_SLICE))?;
    let req = match read_request(&mut stream, &total, &head) {
        Ok(Some(req)) => req,
        Ok(None) => return Ok(()),
        Err((status, message)) => {
            return write_simple(&mut stream, status, &message);
        }
    };
    let (status, body, extra) = route(&handler, &cfg, &req, &*post);
    let mut response = String::new();
    let _ = write!(response, "HTTP/1.1 {status}\r\n");
    for (k, v) in extra {
        let _ = write!(response, "{k}: {v}\r\n");
    }
    match body {
        PostBody::Buffered(body) => {
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
        }
        PostBody::Stream(mut body) => {
            response.push_str(STREAM_HEADERS);
            stream.write_all(response.as_bytes())?;
            body.pump(&mut stream)?;
        }
    }
    finish(&mut stream);
    Ok(())
}

/// Core request routing; returns `(status, body, extra_headers)`.
#[must_use]
fn route<P>(
    handler: &Arc<McpHandler>,
    cfg: &HttpConfig,
    req: &HttpRequest,
    post: &P,
) -> PostResponse
where
    P: PostRoute,
{
    // One verb is served. A `GET` is refused with the same 405 as any other
    // verb: every answer belongs to a POST, so there is nothing for a `GET`
    // to open.
    if let Some(refusal) = crate::method_guard::refuse(&req.method) {
        let extra = refusal
            .headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        return (
            refusal.status,
            PostBody::Buffered(refusal.body.to_owned()),
            extra,
        );
    }
    if let Some(status) = check_origin(cfg, req) {
        return (status, PostBody::Buffered(String::new()), vec![]);
    }
    // Origin first, then credentials: a request from an unapproved Origin never
    // reaches the credential comparison at all.
    let principal = match authorize_request(cfg.credentials.as_deref(), req.get("authorization")) {
        Ok(outcome) => outcome,
        Err(status) => return (status, PostBody::Buffered(String::new()), vec![]),
    };
    if req.path != "/mcp" {
        return (404, PostBody::Buffered(String::new()), vec![]);
    }
    let accepts = req
        .get("accept")
        .map_or_else(String::new, str::to_ascii_lowercase);
    if !accepts_json(&accepts) {
        return (406, PostBody::Buffered(String::new()), vec![]);
    }
    let content_type = req
        .get("content-type")
        .unwrap_or("application/json")
        .to_ascii_lowercase();
    if !content_type.starts_with("application/json") {
        return (415, PostBody::Buffered(String::new()), vec![]);
    }
    // The request-scoped state carries the credential's principal, so dispatch
    // runs through the bound entry point and the shared `Mutex`-held state stays
    // one instance rather than a per-request fork. `Unbound` is the loopback dev
    // default: the base state, unchanged.
    let request_state = scoped_state(handler, principal);
    post.respond(PostContext {
        state: &request_state,
        req,
    })
}

/// Whether this config authorizes anyone over the network.
///
/// A store is usable only when it holds a record: an empty store makes every
/// request a 401, so treating it as a source would only hide that at start.
#[must_use]
pub fn has_credential_source(cfg: &HttpConfig) -> bool {
    cfg.credentials.as_ref().is_some_and(|c| !c.is_empty())
}

/// Whether an `Accept` header admits a JSON answer.
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

/// Half-close the write side, so dropping the socket queues a FIN behind the
/// bytes the peer has already been promised.
///
/// The head promised `Connection: close`, so the FIN belongs here. It is
/// best-effort on the refusal paths, which answer without reading the whole
/// request: a close with the peer's bytes still unread in its receive queue
/// can still be abortive, and the kernel then sends a RST (RFC 9293 3.6.1,
/// RFC 1122 4.2.2.13), which can discard a refusal the peer has not read.
/// Draining would avoid that, but the cap refusal runs in the accept loop, so
/// waiting for a client to stop sending would stall the accept loop. The
/// refusal is best-effort, and the slot is released by `Drop` on every path,
/// read or not.
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
#[path = "http_request_bounds_tests.rs"]
mod http_request_bounds_tests;

#[cfg(test)]
#[path = "http_retention_parity_tests.rs"]
mod http_retention_parity_tests;
#[cfg(test)]
#[path = "http_start_auth_tests.rs"]
mod http_start_auth_tests;
#[cfg(test)]
#[path = "http_stream_tests.rs"]
mod http_stream_tests;
#[cfg(test)]
#[path = "http_test_support_tests.rs"]
pub(crate) mod http_test_support_tests;
#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;
