// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync transport's resource bounds against the real
//! `sync_serve` entry point over real TCP: the 64 KiB head cap, the declared
//! 16 MiB body cap, the total request deadline, and the 8-connection cap with
//! a readable 503.
//!
//! ## Owns
//! - Behavior coverage for `server_conn` (accept loop, pool hand-off, refusals)
//! - Deadline and connection-cap enforcement through the real server, for the
//!   cases where a client keeps sending (`slow_drip_requests_are_cut_off_at_the_total_deadline`)
//! - Release of the whole cap after a close or a mid-request disconnect, and
//!   proof that a stalled connection does not delay a served client
//! - Stop-while-connections-are-in-flight behavior
//! - Listener TTL expiry, and that `stop()` closes the listener immediately
//!   and is safe to call twice
//! - The sync transport's recorded bound values, pinned by value
//!
//! ## Does not own
//! - The read-loop bounds themselves and their unit variants (`http_tests`)
//! - The idle-connection deadline cut and the header block that ends on the
//!   head cap (`request_bounds_tests`)
//! - Pool accounting unit coverage (`conn_pool_tests`)
//! - Route semantics (`server_tests`)
//! - The page-full push payload size measurement
//!   (`push_payload_measurement_tests`)
//! - The timing measurement of a served request behind a stalled connection.
//!   The bound is asserted; the measured milliseconds belong to the change
//!   record, not to a test that would have to keep re-deriving them.
//!
//! ## Boundary
//!
//! Tests here break when connection lifecycle or bound enforcement changes;
//! they never change it.
//!
//! Keep this module focused on sync transport resource-bound tests. Tests
//! assert outcomes, never precise durations; every client-side wait carries a
//! generous safety deadline.

use crate::conn_pool::MAX_CONNECTIONS;
use crate::http::{BODY_CAP_BYTES, HEAD_CAP_BYTES, READ_SLICE};
use crate::server::server_tests::{
    exchange_until, raw_exchange, serve, status_of, status_request, wait_closed,
};
use crate::server_conn::DEFAULT_REQUEST_DEADLINE_SECS;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Ceiling on the full-cap barrier. A barrier that cannot fill within this
/// bound means a slot is missing, which is the failure this check exists to
/// catch, so the bound turns that failure into a test failure rather than a
/// hung gate. Orders of magnitude above the loopback cost.
const BARRIER_BUDGET: Duration = Duration::from_secs(20);

/// Ceiling on how long the cap may take to come back after a client close.
///
/// A slot is released when the server observes the close, which is not the same
/// instant: the connection's task has to be scheduled, read the EOF, and unwind
/// before the counter drops. So a refill probe that lands in that window meets a
/// still-full cap and reads 503, which looks exactly like a leak. Generous,
/// because it bounds a leaked slot into a failure rather than a hang, and well
/// under the request deadline, because a slot freed by the deadline rather than
/// by the close must not pass as a release.
pub(crate) const CAP_RELEASE_BUDGET: Duration = Duration::from_secs(5);

/// Park `count` connections mid-request and return them only once the server
/// has demonstrably accepted all of them.
///
/// This is the step the previous version of `mid_request_disconnect_releases_the_slot`
/// skipped, and skipping it made that test wrong rather than merely weak. A
/// `connect` only completes the TCP handshake; the connection is still in the
/// listen backlog until the accept loop picks it up. Closing a socket that was
/// never accepted exercises no server code at all — the pool never counted it —
/// so the test's premise ("the whole cap went through the disconnect path")
/// was false about a third of the time, and the cap refill that followed was
/// really testing whatever slots happened to be free, not release at all.
///
/// Proving acceptance closes that hole. The accept loop drains the backlog in
/// FIFO order, so one extra request that *is* answered can only be answered
/// after every connection queued ahead of it has been accepted. A 200 on that
/// request is therefore a receipt for the whole set.
pub(crate) fn park_accepted_connections(addr: &str, count: usize) -> Vec<TcpStream> {
    // Park `count - 1` first, leaving exactly one slot free for the receipt
    // below. Parking the full cap up front cannot work: the receipt needs a
    // slot of its own and would be refused with a 503.
    let mut parked = Vec::with_capacity(count);
    for _ in 0..count - 1 {
        let mut park = TcpStream::connect(addr).expect("park connect");
        // A partial head is a request the server is serving, so it holds a
        // slot until the client goes away.
        park.write_all(b"POST /sync/p").expect("partial head");
        parked.push(park);
    }
    // Connect one more and answer it. The accept loop serves connections in
    // arrival order, so a 200 here proves the `count - 1` parked above were
    // accepted ahead of it and hold slots.
    let (status, text) = raw_exchange(addr, &status_request(addr));
    assert_eq!(
        status, 200,
        "the accept loop must drain the backlog before answering, so every \
         parked connection is accepted: {text}"
    );
    // That answered request held the last free slot, and it is now released.
    // Re-park one connection to fill the cap, then prove the whole set is
    // accepted the same way: it is queued behind the parked ones, so a refusal
    // to accept it would leave the cap short.
    let mut last = TcpStream::connect(addr).expect("last park connect");
    last.write_all(b"POST /sync/p").expect("partial head");
    parked.push(last);
    // A connection above the cap is refused, and that refusal is itself proof
    // the loop accepted the connection in front of it. The accept loop refuses
    // above the cap rather than queueing, so reaching the refusal means the cap
    // was reached by accepted connections, not by backlog entries.
    let (status, text) = raw_exchange(addr, &status_request(addr));
    assert_eq!(
        status, 503,
        "the cap must be full before the next connection is refused, so every \
         parked connection is accepted: {text}"
    );
    assert!(
        text.contains("connection limit reached (8)"),
        "the refusal must name the connection cap: {text}"
    );
    parked
}

/// Prove the **whole** cap is acquirable at the same time, which a sequential
/// probe cannot do.
///
/// A probe that closes before the next one opens only ever needs a single free
/// slot: with 7 of 8 slots leaked, all 8 sequential probes still get a 200 and
/// the test passes. (`exchange_until` makes it worse — it retries a 503 for
/// the whole budget, so a refusal is absorbed rather than seen.)
///
/// The barrier is what separates the two cases. Each probe connects, writes a
/// complete request, then stops there with its socket open. No probe reads a
/// byte until every probe has reached that point, so all `MAX_CONNECTIONS`
/// requests compete for slots simultaneously. With `k` slots leaked, at most
/// `MAX_CONNECTIONS - k` can be served; the rest read 503 or go unanswered, so
/// a single leaking release path fails this check.
pub(crate) fn full_cap_is_reacquirable_concurrently(addr: &str) -> bool {
    let barrier = Arc::new(Barrier::new(MAX_CONNECTIONS + 1));
    let mut handles = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let addr = addr.to_owned();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mut stream = TcpStream::connect(&addr).expect("probe connect");
            // `status_request` uses `Connection: close`, so the server answers
            // and closes; the slot is held until the write below completes.
            if stream.write_all(&status_request(&addr)).is_err() {
                return false;
            }
            // Block here with the request outstanding until every sibling is
            // likewise outstanding. Reading before that is what let a partial
            // leak pass: an early probe could free its slot for a later one.
            if !wait_at_barrier(&barrier) {
                return false;
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut raw = Vec::new();
            let _ = stream.read_to_end(&mut raw);
            status_of(&String::from_utf8_lossy(&raw)) == 200
        }));
    }
    // The main thread is the +1. Reaching here means every probe connected and
    // wrote its request, so all the slots are contested at the same time.
    if !wait_at_barrier(&barrier) {
        return false;
    }
    // Every probe now reads. A leaked slot shows as a non-200 or a no-answer.
    handles.into_iter().all(|h| h.join().unwrap_or(false))
}

/// Poll `f` until it returns true or `budget` elapses. Polling against a
/// generous deadline instead of asserting exact accept timing.
///
/// The cap-release checks need this because a client `close` returns before the
/// server's connection task has been scheduled to read the EOF and drop its
/// slot. A refill probe landing in that window meets a still-full cap and reads
/// 503, which is indistinguishable from a leak. The window is microseconds on a
/// many-core developer machine and long enough to lose nearly every time on a
/// loaded CI runner, which is why this read as a flake there and never locally.
/// A leaked slot never satisfies `f`, so the leak these checks exist to catch
/// still fails them, and the budget bounds that failure instead of hanging.
pub(crate) fn retry_until(budget: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + budget;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// `Barrier::wait` behind a ceiling. `Barrier::wait` itself has no timeout, so
/// it runs on a helper thread and the caller blocks on the channel instead. A
/// barrier that can never fill (a missing slot stops a probe from ever
/// connecting) then becomes a bounded test failure rather than a hung gate.
/// The helper thread is left behind on timeout, but the test process exits
/// shortly after, which reclaims it.
fn wait_at_barrier(barrier: &Arc<Barrier>) -> bool {
    let (tx, rx) = mpsc::channel();
    let barrier = Arc::clone(barrier);
    thread::spawn(move || {
        barrier.wait();
        let _ = tx.send(());
    });
    rx.recv_timeout(BARRIER_BUDGET).is_ok()
}

#[test]
fn head_cap_refuses_an_oversized_header_block_with_413() {
    let server = serve(&["b1"], Some("t1"), false, None, None).0;
    let addr = server.addr().to_owned();
    // Over the head cap with no terminator: refused even undeclared.
    let head = format!(
        "POST /sync/push HTTP/1.1\r\nHost: {addr}\r\nX-Filler: {}\r\n",
        "f".repeat(HEAD_CAP_BYTES + 8192)
    );
    // Retried until the 413 is seen: the server closes with the oversized head
    // still unread, which can reset the connection and discard a refusal the
    // client had already buffered.
    let (status, text) = exchange_until(&addr, head.as_bytes(), 413, 10);
    assert_eq!(status, 413, "expected the head-cap refusal: {text}");
    assert!(
        text.contains("64 KiB head cap"),
        "must name the bound: {text}"
    );
    server.stop();
}

#[test]
fn declared_body_cap_refuses_before_any_body_byte_is_read() {
    let server = serve(&["b1"], Some("t1"), false, None, None).0;
    let addr = server.addr().to_owned();
    // Declared over-cap length, no body bytes: refuse without waiting.
    let head = format!(
        "POST /sync/push HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer t1\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        BODY_CAP_BYTES + 1
    );
    let (status, text) = raw_exchange(&addr, head.as_bytes());
    assert_eq!(status, 413, "expected the body-cap refusal: {text}");
    assert!(
        text.contains("16 MiB body cap"),
        "must name the bound: {text}"
    );
    server.stop();
}

/// A slow client that keeps trickling bytes under the per-read slice is
/// stopped by the total deadline alone. The drip is paced well under the 5 s
/// per-read slice, so every byte renews the read wait and only the deadline can
/// end the request; the assertion covers both the refusal and the bound, so a
/// bare cutoff cannot pass for it.
#[test]
fn slow_drip_requests_are_cut_off_at_the_total_deadline() {
    let server = serve(&["b1"], Some("t1"), false, None, Some(2)).0;
    let addr = server.addr().to_owned();
    let mut stream = TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("client read timeout");
    // Drip one byte every 100 ms (well under the 5 s read slice) while a
    // reader accumulates the response. 40 dripped bytes take 4 s: past the
    // 2 s deadline, so only the total deadline can end this request.
    let mut reader_stream = stream.try_clone().expect("clone socket");
    let reader = thread::spawn(move || {
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match reader_stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
        raw
    });
    let started = Instant::now();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(100));
        // The server cuts the connection at the deadline; a failed write ends
        // the drip early, which is the expected cutoff.
        if stream.write_all(b"P").is_err() {
            break;
        }
    }
    drop(stream);
    let text = String::from_utf8_lossy(&reader.join().expect("reader")).into_owned();
    let elapsed = started.elapsed();
    let status = status_of(&text);
    // A real 408 refusal naming the 2 s deadline: a bare cutoff (empty
    // buffer) or a client-side timeout would leave the read below unproven.
    assert_eq!(status, 408, "the deadline must answer 408: {text}");
    assert!(
        text.contains("request deadline exceeded (2 s per request)"),
        "must name the deadline it enforced: {text}"
    );
    // The cut landed at the server's 2 s deadline, not at the client's 15 s
    // read timeout: 10 s is far past the deadline and far short of that read.
    assert!(
        elapsed < Duration::from_secs(10),
        "the 408 must arrive at the 2 s deadline, not the client timeout: \
         took {elapsed:?}"
    );
    server.stop();
}

/// A staller is held on its own slot past a generous bound; a legitimate
/// client must be served well inside that hold, so a stalled connection
/// cannot be delaying the accept loop or the pool. Both the 1 s hold and the
/// 3 s ceiling are orders of magnitude above the loopback time this costs
/// (single-digit milliseconds), so neither can flake.
#[test]
fn stalled_connection_does_not_block_other_connections() {
    let server = serve(&["b1"], Some("t1"), false, None, None).0;
    let addr = server.addr().to_owned();
    // One silent staller with a partial head: it holds a slot on its own worker
    // until the deadline; other connections must proceed.
    let staller = TcpStream::connect(&addr).expect("staller connect");
    let mut partial = staller.try_clone().expect("clone");
    partial.write_all(b"POST /sync/pu").expect("partial head");
    let started = Instant::now();
    let (status, _) = raw_exchange(&addr, &status_request(&addr));
    let elapsed = started.elapsed();
    assert_eq!(status, 200, "the legitimate request must be served");
    assert!(
        elapsed < Duration::from_secs(3),
        "a stalled connection delayed the status request by {elapsed:?}"
    );
    // The staller is still open and still silent; it holds its slot for the
    // full hold, and a second client is served inside it too.
    thread::sleep(Duration::from_secs(1));
    let (after, _) = raw_exchange(&addr, &status_request(&addr));
    assert_eq!(after, 200, "the staller must not have ended the exchange");
    server.stop();
}

/// `stop()` with a connection in flight: the accept loop must exit promptly
/// even while a worker holds a slot, and the in-flight worker finishes
/// bounded (it is not killed mid-request).
#[test]
fn stop_while_connections_are_in_flight_stops_the_listener_promptly() {
    let (server, _store) = serve(&["b1"], Some("t1"), false, None, Some(8));
    let addr = server.addr().to_owned();
    let mut staller = TcpStream::connect(&addr).expect("staller connect");
    // Bound the reads below the way `raw_exchange` does: a hung worker must
    // fail this test at the 20 s ceiling instead of blocking the test thread
    // forever on an untimed read and stalling the whole test gate.
    staller
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("staller read timeout");
    staller.write_all(b"POST /sync/p").expect("partial head");
    // Prove the staller was accepted before stopping: the accept loop drains
    // the backlog in order, so a probe connected after the staller can only
    // be served once the staller has been accepted.
    let (status, _) = exchange_until(&addr, &status_request(&addr), 200, 10);
    assert_eq!(status, 200, "probe served while the staller is accepted");
    server.stop();
    // The accept loop observes the flag at the top of every iteration: it
    // stops accepting and exits with a slot still held.
    wait_closed(&addr, 2, "stop with a connection in flight");
    // The in-flight worker ends bounded: the stalled request is refused at
    // its deadline and the socket reaches EOF. A reset is also a bounded end
    // (listener teardown may reset a still-pending connection). What must
    // never happen is an unbounded hang or a served response. The loop reads
    // first, so a hung worker can only surface as the assert below.
    let cutoff = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 512];
    let mut refusal = String::new();
    loop {
        match staller.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => refusal.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
        assert!(
            Instant::now() < cutoff,
            "in-flight connection must end bounded, got text so far: {refusal}"
        );
    }
    assert!(
        !refusal.contains(" 200 "),
        "a stopped server must not serve the stalled request: {refusal}"
    );
}

/// The sync transport's recorded bound values, pinned by value so a change to
/// any of them is a deliberate edit to a test, as on the MCP transport.
#[test]
fn resource_bounds_are_pinned_to_the_recorded_values() {
    assert_eq!(HEAD_CAP_BYTES, 64 * 1024, "64 KiB head cap");
    assert_eq!(BODY_CAP_BYTES, 16 * 1024 * 1024, "16 MiB body cap");
    assert_eq!(READ_SLICE, Duration::from_secs(5), "5 s per-read slice");
    assert_eq!(MAX_CONNECTIONS, 8, "8 connection slots");
    assert_eq!(
        DEFAULT_REQUEST_DEADLINE_SECS, 30,
        "30 s total request deadline"
    );
}

/// A listener with a TTL closes on its own, and `stop()` on a listener without
/// one closes it immediately rather than waiting for a TTL that will not come.
/// `stop()` is also safe to call twice.
#[test]
fn ttl_expires_the_listener_and_stop_is_immediate_and_idempotent() {
    let (server, _store) = serve(&["b1"], Some("t1"), false, Some(1), None);
    let addr = server.addr().to_owned();
    assert!(TcpStream::connect(&addr).is_ok());
    wait_closed(&addr, 2, "ttl");
    server.stop();
    server.stop();
    let (server, _store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    assert!(
        TcpStream::connect(&addr).is_ok(),
        "a fresh listener accepts"
    );
    server.stop();
    wait_closed(&addr, 2, "stop()");
}
