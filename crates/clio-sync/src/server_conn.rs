// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's connection lifecycle: the
//! non-blocking accept loop with its 50 ms `WouldBlock` sleep, bounded pool
//! slot acquisition with a readable 503 refusal above the cap, the total
//! request deadline (default [`DEFAULT_REQUEST_DEADLINE_SECS`], deployment
//! raise/lower knob `SyncServerOptions::deadline_secs`), and the refusal
//! writers that map read-bound failures to statuses (413 oversize head/body,
//! 408 deadline, 503 connection cap).
//!
//! ## Owns
//! - The accept loop (`serve_loop`), per-connection handling, and the pool
//!   slot hand-off (one worker thread per accepted connection)
//! - Refusal status mapping and wording: 413 head/body cap, 408 deadline,
//!   503 connection limit; refusal messages name the bound and its value
//! - The response writers shared with the route handlers (`write_json`,
//!   `write_status_error`) and the short write bound on the above-cap refusal
//!   (`REFUSAL_WRITE_SLICE`), so no write runs unbounded in the accept loop
//! - The default total request deadline (30 s) and its per-connection start
//!
//! ## Does not own
//! - The bound constants and their in-loop enforcement (`http`)
//! - The pool accounting and cap value (`conn_pool`)
//! - Route semantics: push/pull/status handlers and auth of banks (`server`)
//! - The request's credential check is enforced here but resolution lives in
//!   `credential` and the material in `ServeOptions` (`server`)
//!
//! ## Boundary
//!
//! Changes to how connections are accepted, bounded, refused, or torn down
//! should normally require changes here. Changes to apply semantics or the
//! route payloads should not require changes here.
//!
//! Keep this module focused on the connection lifecycle.
//!
//! ## Stop and lifetime contract
//!
//! `stop` is polled at the top of the accept loop: on stop the loop stops
//! accepting and exits. In-flight connection workers are not killed: each
//! finishes its reads either at its own total request deadline or sooner on
//! disconnect. Work after the read is not preempted, so a worker that reached
//! an apply can outlive its deadline by an unbounded amount; a stopped
//! server's workers therefore do not die instantly, and a worker is not
//! guaranteed to stop at its deadline.
//!
//! # Why the write side is shut down
//!
//! Every response here is written and then the socket is dropped, and for two
//! refusal paths — the connection cap and the head cap — the request was never
//! fully read, so the peer's bytes are still sitting unread in the receive
//! queue. Closing a socket in that state is an *abortive* close: RFC 9293
//! (ABORT) sends a RST where CLOSE sends a FIN, and requires a graceful
//! close to keep reading "until the TCP implementation indicates there is no
//! more data". The kernel does not read for us, so the close is abortive.
//!
//! The reset does not merely replace the response, it discards it: a peer that
//! has not yet read the bytes sitting in its own receive buffer loses them, and
//! reads a truncated response followed by `ECONNRESET`. That is what made the cap
//! and head-cap refusals unreadable — the message naming the limit was written,
//! and then thrown away by the transport on the way out.
//!
//! Draining the peer's bytes first would also work, but it cannot be done on
//! these paths: the cap refusal runs inside the accept loop, so a drain that
//! waits for a client to finish sending is exactly the stall that issues #22 and
//! #23 were filed about. Shutting down only the write side queues a FIN behind
//! the payload the peer has already been promised, costs one syscall, and turns
//! the close back into a graceful one. Measured on this host over 20 repetitions
//! of a request-then-refuse exchange: a bare close delivered the body 10 times
//! out of 20, and a write-side shutdown delivered it 20 times out of 20.

use crate::conn_pool::{ConnectionPool, MAX_CONNECTIONS};
use crate::credential::authorize_request;
use crate::http::{
    BODY_CAP_BYTES, HEAD_CAP_BYTES, READ_SLICE, ReadRequestError, Route, io_read, read_request,
};
use crate::server::{ServeOptions, handle_pull, handle_push, handle_status};
use clio_types::{AmError, ErrorCode};
use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Default total request deadline for one request, measured from the first
/// accepted byte toward the response: a slow-dripping client is cut off at
/// the same point a silent client is. Deployments raise or lower it via
/// `SyncServerOptions::deadline_secs`.
pub(crate) const DEFAULT_REQUEST_DEADLINE_SECS: u64 = 30;

/// Write timeout for the above-cap connection refusal, kept short so a peer
/// that never reads its 503 cannot stall the accept loop. The refusal is
/// written before any per-connection write timeout is installed, so it needs
/// its own bound.
const REFUSAL_WRITE_SLICE: Duration = Duration::from_secs(2);

/// Shared accept-loop context: the listener, stop flag, serve options, TTL,
/// and the bounded connection pool.
pub(crate) struct ServeContext {
    /// Bound (non-blocking) listener.
    pub(crate) listener: TcpListener,
    /// Stop flag polled at the top of the accept loop.
    pub(crate) stop: Arc<AtomicBool>,
    /// Route-handler options (also carries the request deadline).
    pub(crate) options: Arc<ServeOptions>,
    /// Auto-stop TTL seconds.
    pub(crate) ttl_secs: u64,
    /// When serving started (TTL anchor).
    pub(crate) started: Instant,
    /// Bounded connection pool; a full pool refuses with 503.
    pub(crate) pool: Arc<ConnectionPool>,
}

/// Refusal body: `ok: false` with a code string and a message that names the
/// bound and its value. Never echoes request bodies or Authorization values.
fn refusal_body(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "ok": false, "code": code, "message": message })
}

fn head_cap_message() -> String {
    format!(
        "header block exceeds the {} KiB head cap",
        HEAD_CAP_BYTES / 1024
    )
}

fn body_cap_message() -> String {
    format!(
        "declared body exceeds the {} MiB body cap",
        BODY_CAP_BYTES / (1024 * 1024)
    )
}

fn deadline_message(secs: u64) -> String {
    format!("request deadline exceeded ({secs} s per request)")
}

fn connection_limit_message() -> String {
    format!("connection limit reached ({MAX_CONNECTIONS})")
}

/// Refuse a connection accepted above the connection cap with a readable 503
/// and close. This runs in the accept loop, before any per-connection write
/// timeout exists, so the write is bounded here: a peer that never reads its
/// refusal must not stall accepting. The refusal is best-effort by decision —
/// a failed timeout setup or a failed write just drops the connection — so
/// both results are deliberately discarded.
fn refuse_over_cap(stream: TcpStream) -> Result<(), AmError> {
    let _ = stream.set_write_timeout(Some(REFUSAL_WRITE_SLICE));
    write_json(
        stream,
        503,
        &refusal_body("service_unavailable", &connection_limit_message()),
    )
}

/// Accept loop: non-blocking accept with a 50 ms `WouldBlock` sleep; the stop
/// flag is polled at the top of every iteration. Each accepted connection
/// takes one pool slot and runs on its own worker thread; when the pool is
/// full the connection is refused with a readable 503 and closed.
pub(crate) fn serve_loop(ctx: &ServeContext) {
    loop {
        if ctx.stop.load(Ordering::SeqCst)
            || ctx.started.elapsed() > Duration::from_secs(ctx.ttl_secs)
        {
            return;
        }
        match ctx.listener.accept() {
            Ok((stream, _)) => {
                // The accepted stream inherits the listener's non-blocking
                // mode on some platforms; every read below relies on blocking
                // behavior with timeouts, so this setup failure is a broken
                // invariant and is named here rather than discarded.
                stream
                    .set_nonblocking(false)
                    .expect("accepted stream must return to blocking mode");
                match ctx.pool.try_acquire() {
                    Some(slot) => {
                        let options = Arc::clone(&ctx.options);
                        std::thread::Builder::new()
                            .name("clio-sync-conn".into())
                            .spawn(move || {
                                let _slot = slot; // released by Drop on every exit path
                                let _ = handle_connection(&options, stream);
                            })
                            .expect("connection worker spawn");
                    }
                    None => {
                        let _ = refuse_over_cap(stream);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return,
        }
    }
}

/// Handle one connection from first byte to response. The total request
/// deadline starts here, at the first accepted byte.
fn handle_connection(options: &ServeOptions, mut stream: TcpStream) -> Result<(), AmError> {
    let deadline = Instant::now() + Duration::from_secs(options.deadline_secs);
    // Bound response writes too: a client that never reads must not hold its
    // connection slot on a stalled write.
    stream.set_write_timeout(Some(READ_SLICE)).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("write timeout setup failed: {e}"),
        )
    })?;
    let request = match read_request(&mut stream, deadline) {
        Ok(request) => request,
        Err(ReadRequestError::OversizeHead) => {
            return write_json(
                stream,
                413,
                &refusal_body("payload_too_large", &head_cap_message()),
            );
        }
        Err(ReadRequestError::OversizeBody) => {
            return write_json(
                stream,
                413,
                &refusal_body("payload_too_large", &body_cap_message()),
            );
        }
        Err(ReadRequestError::Deadline) => {
            return write_json(
                stream,
                408,
                &refusal_body("request_timeout", &deadline_message(options.deadline_secs)),
            );
        }
        // Pure I/O errors close the connection silently, as before the
        // bounds existed: there is no meaningful response to send.
        Err(ReadRequestError::Io(e)) => return Err(io_read(e)),
    };
    // One deadline check, here: receiving is bounded by the loop in
    // `read_request`, and this catches a request whose read finished so close
    // to the deadline that the work has already spent. There is deliberately no
    // check after the handler runs — a push whose apply overruns finishes and
    // is answered with its normal acknowledgement, not refused after the fact.
    if Instant::now() >= deadline {
        return write_json(
            stream,
            408,
            &refusal_body("request_timeout", &deadline_message(options.deadline_secs)),
        );
    }
    let Some(route) = request.route else {
        return write_json(
            stream,
            404,
            &serde_json::json!({ "ok": false, "code": "not_found", "message": "unknown route" }),
        );
    };
    // Credentials are resolved once, here, after route resolution and before
    // dispatch: the handlers read the scope this request was bound to, and no
    // other path reaches a handler.
    let auth = match authorize_request(
        options.credentials.as_ref(),
        options.dev_mode,
        request.authorization.as_deref(),
    ) {
        Ok(auth) => auth,
        Err(_status) => {
            return write_json(
                stream,
                401,
                &serde_json::json!({ "ok": false, "code": "forbidden", "message": "unauthorized" }),
            );
        }
    };
    match route {
        Route::Push => handle_push(options, stream, &request.body, &auth),
        Route::Pull => handle_pull(options, stream, &request.body, &auth),
        Route::Status => handle_status(options, stream, &auth),
    }
}

/// Write one JSON response with a `Connection: close` head.
///
/// The write side is shut down before the socket is dropped. See the module's
/// "Why the write side is shut down" note: dropping a socket that still has the
/// peer's request bytes unread in its receive queue aborts the connection, and
/// the reset discards the response the peer has not read yet.
pub(crate) fn write_json(
    mut stream: TcpStream,
    status: u16,
    body: &serde_json::Value,
) -> Result<(), AmError> {
    // Serializing a `serde_json::Value` cannot fail.
    let payload = serde_json::to_vec(body).expect("serialize response");
    let status_text = if status == 200 { "OK" } else { "Error" };
    let head = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&payload))
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("write failed: {e}")))?;
    let _ = stream.flush();
    // The head already promised `Connection: close`, so the FIN belongs here: it
    // is queued behind the payload, so the peer reads the response first and the
    // close afterwards. A best-effort call by decision — a peer that has already
    // gone away makes this fail, and there is nothing left to say to it.
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

pub(crate) fn write_status_error(
    stream: TcpStream,
    status: u16,
    err: &AmError,
) -> Result<(), AmError> {
    write_json(
        stream,
        status,
        &serde_json::json!({ "ok": false, "code": err.code.as_str(), "message": err.message }),
    )
}

#[cfg(test)]
#[path = "server_conn_tests.rs"]
pub(crate) mod server_conn_tests;
