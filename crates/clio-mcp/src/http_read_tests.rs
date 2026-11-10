// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves `read_request` at its own layer: on a real loopback
//! socket pair, a head read under a short header budget ends in a 408 naming
//! that budget, and a head that arrives inside the budget is served.
//!
//! ## Owns
//! - The header-deadline outcome of `read_request`, driven directly on a
//!   loopback socket pair on an OS-assigned port
//!
//! ## Does not own
//! - The response bytes and the connection slots (see `http` and `conn_pool`)
//! - The bound values themselves (defined and documented in `http`)
//!
//! ## Boundary
//!
//! Changes to how `read_request` charges a head read to its budget should
//! normally require changes here. Changes to routing, response writing, or
//! connection accounting should not require changes here.

use super::{ReadBound, read_request};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// Total deadline used by both tests. Far above the 1 s header budget, so only
/// the header budget can end a head read, and far above the loopback cost, so
/// a loaded host cannot flake the pass.
const TOTAL_SECS: u64 = 30;

/// Header budget under test. Long enough for a complete head on loopback, so
/// the served case is not a race, and short enough that an expiry is quick.
const HEAD_SECS: u64 = 1;

/// Ceiling on the wait for the header budget to expire. An order of magnitude
/// above the budget, so a budget that never fires fails instead of hanging.
const HEAD_EXPIRY_CEILING: Duration = Duration::from_secs(10);

/// Total deadline for the starved-body case. Short so the red proof (a head
/// budget equal to the total, which disables the body-progress bound) fails in
/// seconds instead of waiting the 30 s total, yet far above the loopback cost
/// of the complete head that starts it.
const STARVED_BODY_TOTAL_SECS: u64 = 5;

/// Body-progress budget under test: the same 1 s slice the head uses.
const STARVED_BODY_HEAD_SECS: u64 = 1;

/// A connected loopback socket pair on an OS-assigned port, with a generous
/// receive timeout on the reading side so a stuck read fails the test instead
/// of hanging it.
fn socket_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind an OS-assigned port");
    let addr = listener.local_addr().expect("listener address");
    let client = TcpStream::connect(addr).expect("connect to the listener");
    let (server, _) = listener.accept().expect("accept the client");
    server
        .set_read_timeout(Some(HEAD_EXPIRY_CEILING))
        .expect("receive timeout");
    (client, server)
}

/// The bounds a served request is read under: a 30 s total and the 1 s header
/// slice inside it.
fn bounds(now: Instant) -> (ReadBound, ReadBound) {
    (
        ReadBound::total(now, TOTAL_SECS),
        ReadBound::head(now, TOTAL_SECS, HEAD_SECS),
    )
}

#[test]
fn partial_head_fails_at_the_header_deadline_with_408() {
    let (mut client, mut server) = socket_pair();
    // A head with no terminator, written in one go. The client then stays
    // open and silent, so the header budget is the only thing that can end
    // the read.
    client
        .write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\n")
        .expect("write a partial head");
    let (total, head) = bounds(Instant::now());
    let started = Instant::now();
    let outcome = read_request(&mut server, &total, &head);
    let elapsed = started.elapsed();
    let Err((status, message)) = outcome else {
        panic!("a partial head must not be read as a request");
    };
    assert_eq!(status, 408, "an expired header budget is a request timeout");
    assert!(
        message.contains("1 s header read deadline"),
        "the refusal must name the header budget and its value: {message}"
    );
    assert!(
        elapsed < HEAD_EXPIRY_CEILING,
        "the 1 s header budget must end the read long before the 30 s total: {elapsed:?}"
    );
}

#[test]
fn complete_head_inside_the_header_deadline_is_served() {
    let (mut client, mut server) = socket_pair();
    client
        .write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .expect("write a complete head");
    let (total, head) = bounds(Instant::now());
    let Ok(Some(request)) = read_request(&mut server, &total, &head) else {
        panic!("a head sent inside the header budget must be served");
    };
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/mcp"),
        "the served request must be parsed from the head that was in budget"
    );
}

#[test]
fn starved_body_fails_at_the_body_progress_deadline_with_408() {
    let (mut client, mut server) = socket_pair();
    // A complete head that declares one body byte. The client then stays open
    // and silent, so only the body-progress bound can end the read. Without
    // that bound the connection would hold its slot for the whole total.
    client
        .write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\n\r\n")
        .expect("write a complete head");
    let now = Instant::now();
    let total = ReadBound::total(now, STARVED_BODY_TOTAL_SECS);
    let head = ReadBound::head(now, STARVED_BODY_TOTAL_SECS, STARVED_BODY_HEAD_SECS);
    let started = Instant::now();
    let outcome = read_request(&mut server, &total, &head);
    let elapsed = started.elapsed();
    let Err((status, message)) = outcome else {
        panic!("a starved body must not be read as a request");
    };
    assert_eq!(
        status, 408,
        "an expired body-progress budget is a request timeout"
    );
    assert!(
        message.contains("1 s body read deadline"),
        "the refusal must name the body-progress budget and its value: {message}"
    );
    assert!(
        elapsed < HEAD_EXPIRY_CEILING,
        "the 1 s body-progress budget must end the read long before the 5 s total: {elapsed:?}"
    );
}
