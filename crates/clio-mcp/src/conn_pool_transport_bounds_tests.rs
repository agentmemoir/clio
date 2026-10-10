// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP transport's connection bound against the real
//! `serve_with_listener` entry point over real TCP: the 32-connection cap with
//! its readable 503 refusals, and the release of the **whole** cap after idle
//! connections or mid-request disconnects give theirs back.
//!
//! ## Owns
//! - Socket-level transport tests driven through the real
//!   `http::serve_with_listener` on an OS-assigned port
//! - The full-cap reacquisition proof, which is why this module exists apart
//!   from `conn_pool_tests`: the unit pool tests exercise the `Drop` guard
//!   only, so a leaking release path inside a worker is invisible to them
//!
//! ## Does not own
//! - Slot counting and the `Drop` release themselves (see `conn_pool`)
//! - Request parsing and the bound values (see `http_read` and `http`)
//! - Pool unit tests, routing/session status codes, and the byte and deadline
//!   bounds (`conn_pool_tests`, `http_tests`, `request_bounds_tests`)
//!
//! ## Boundary
//!
//! Changes to how connection slots are released or bounded by the transport
//! should normally require changes here. Changes to routing, sessions, or tool
//! behavior should not require changes here.
//!
//! Keep this module focused on transport resource-bound tests. Tests assert
//! outcomes and generous bounds, never precise durations.

use crate::conn_pool::MAX_CONNECTIONS;
use crate::http::{HttpConfig, serve_with_listener};
use crate::http_auth::CredentialStore;
use crate::runtime::{McpOpenOptions, McpState};
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;

/// Ceiling on the full-cap barrier. A barrier that cannot fill within this
/// bound means a slot is missing, which is exactly the failure this check
/// exists to catch, so the bound turns that failure into a test failure
/// instead of a hung gate. Orders of magnitude above the loopback cost.
const BARRIER_BUDGET: Duration = Duration::from_secs(20);

/// Serve through the real `serve_with_listener` in a background thread. The
/// thread runs until the per-test process exits, which matches the production
/// loop shape.
fn spawn_server(cfg: HttpConfig) -> u16 {
    let state = McpState::open(&McpOpenOptions::default()).expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let _ = serve_with_listener(state, cfg, listener);
    });
    port
}

fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).expect("connect")
}

fn init_request(extra: &str) -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\n{extra}\r\n{INIT}",
        INIT.len()
    )
}

/// One `initialize` POST over a fresh connection; returns `(status, body)` when
/// a complete response arrived. A failed write does not bail: the server may
/// already have answered (for example with the above-cap refusal).
fn probe_init(port: u16, extra: &str) -> Option<(u16, String)> {
    let mut stream = connect(port);
    let _ = stream.write_all(init_request(extra).as_bytes());
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    parse_response(&buf)
}

/// Split a raw response into `(status, body)`; `None` without a full head.
fn parse_response(buf: &[u8]) -> Option<(u16, String)> {
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..pos]);
    let status = head
        .split("\r\n")
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    Some((status, body))
}

/// Poll `f` until it returns true or `budget` elapses. Polling against a
/// generous deadline instead of asserting exact accept timing.
fn retry_until(budget: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + budget;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Park `count` idle connections and return them only once the server has
/// demonstrably accepted all of them.
///
/// A `connect` only completes the TCP handshake; the connection is still in the
/// listen backlog until the accept loop picks it up. Dropping a socket that was
/// never accepted exercises no server code at all — the pool never counted it —
/// so a holder set built from raw `connect` calls does not actually saturate
/// anything, and the slot-release check that follows it re-checks whatever
/// slots happened to be free. That is exactly how this test passed a 50% leak
/// mutant on the idle-close path until the set was proven accepted.
///
/// Proof is the cap refusal itself: the accept loop refuses above the cap rather
/// than queueing, so a connection that reads a 503 naming the cap can only have
/// been refused *after* every slot below it was taken by an accepted
/// connection. Parking `count - 1` leaves a slot free for the probe; the extra
/// idle connection then takes it, and the refusal that follows is the receipt.
fn park_accepted_idle_connections(port: u16, count: usize) -> Vec<TcpStream> {
    // Open `count` plus a few spares, so the cap is reachable whatever order
    // the accept loop takes them in.
    let mut idle: Vec<TcpStream> = (0..count + 8).map(|_| connect(port)).collect();
    // The cap refusal is the receipt. The accept loop refuses above the cap
    // rather than queueing, so a probe that reads a 503 naming the cap can only
    // have been refused *after* every slot was taken by an accepted connection:
    // at least `count` of the sockets above are accepted and holding.
    let refused = retry_until(Duration::from_secs(10), || {
        matches!(
            probe_init(port, ""),
            Some((503, body)) if body.contains("connection limit reached (32)")
        )
    });
    assert!(
        refused,
        "a probe above the cap must read a 503 naming the cap, which is the \
         receipt that at least {count} of the parked connections were accepted"
    );
    // The refused probe released nothing it held, so the cap is still held by
    // the accepted connections above. Trim to exactly `count` for the release
    // check: dropping extras is safe, and a leaked slot cannot be masked by a
    // holder that is still open.
    idle.truncate(count);
    idle
}

/// Open `count` connections, write a partial request on each, and return them
/// open. Every connection here is a request the server is *serving*, not
/// refusing, so each one holds a slot until it is dropped.
fn saturate_with_partials(port: u16, count: usize) -> Vec<TcpStream> {
    let mut held = Vec::with_capacity(count);
    for _ in 0..count {
        let mut stream = connect(port);
        // A partial head: the server is waiting for the rest of the request,
        // so this connection owns a slot and holds it until the client goes
        // away or the deadline expires.
        let _ = stream.write_all(b"POST /mcp HTT");
        held.push(stream);
    }
    held
}

/// Prove the **whole** cap is acquirable at the same time, which a sequential
/// probe cannot do.
///
/// A sequential probe that closes before the next opens only ever needs a
/// single free slot: with 31 of 32 slots leaked, all 32 probes still get a 200
/// and the test passes.
///
/// The barrier is what separates the two cases. Each probe connects, writes a
/// complete request, then stops there with its socket open. No probe reads a
/// byte until every probe has reached that point, so all `MAX_CONNECTIONS`
/// requests compete for slots at the same moment. With `k` slots leaked, at
/// most `MAX_CONNECTIONS - k` can be served; the rest read 503 or go
/// unanswered, and a single leaking release path fails the check.
fn full_cap_is_reacquirable_concurrently(port: u16, auth: &str) -> bool {
    // A `Barrier` is the natural fit: every probe waits on it after writing
    // its request, so none can read a byte until all of them are connected and
    // outstanding. The wait is a synchronization, not a poll, so it cannot
    // return early on a loaded host.
    //
    // `Barrier::wait` has no timeout, so each probe runs inside `wait_timeout`
    // from a helper thread: a barrier that can never fill (a missing slot
    // stops a probe from ever connecting) turns into a bounded failure rather
    // than a hung test gate.
    let barrier = Arc::new(Barrier::new(MAX_CONNECTIONS + 1));
    let mut handles = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let (port, auth) = (port, auth.to_owned());
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut stream = connect(port);
            if stream.write_all(init_request(&auth).as_bytes()).is_err() {
                return None;
            }
            // Block here with the request outstanding until every sibling is
            // likewise outstanding. Reading before that is what let a partial
            // leak pass: an early probe could free its slot for a later one.
            if !wait_at_barrier(&barrier) {
                return None;
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            parse_response(&buf)
        }));
    }
    // The main thread is the +1. Reaching here means every probe connected and
    // wrote its request, so all the slots are contested at the same time.
    if !wait_at_barrier(&barrier) {
        return false;
    }
    // Every probe now reads. A leaked slot shows as a non-200 or a no-answer.
    let mut all_ok = true;
    for handle in handles {
        match handle.join() {
            Ok(Some((200, _))) => {}
            _ => all_ok = false,
        }
    }
    all_ok
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
    std::thread::spawn(move || {
        barrier.wait();
        let _ = tx.send(());
    });
    rx.recv_timeout(BARRIER_BUDGET).is_ok()
}

#[test]
fn connection_cap_refuses_above_cap_and_releases_on_close() {
    // The store record is the credential the probe requests carry, so the
    // served probes pass the credential edge like real traffic would.
    let store = CredentialStore::from_records(vec![AuthTokenRecord {
        label: "ci".into(),
        secret_sha256: digest_hex("secret-token"),
        actor: Actor::Harness,
        banks: vec!["demo".to_owned()],
        created_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let cfg = HttpConfig {
        credentials: Some(Arc::new(store)),
        // Longer than the probe windows below, so a pass proves close-driven
        // release rather than deadline-driven slot recycling.
        deadline_secs: Some(30),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    let auth = "Authorization: Bearer secret-token\r\n";
    // Idle connections each hold a slot without ever completing a request. The
    // set is proven accepted before it is used, or the release check below
    // would only prove that some slots were free.
    let idle = park_accepted_idle_connections(port, MAX_CONNECTIONS);
    drop(idle);
    // Polled within a budget rather than asked once. `drop` returns before the
    // server's connection tasks have been scheduled to read the EOF and release
    // their slots, so a probe landing in that window reads a still-full cap and
    // looks exactly like a leak. A leaked slot never satisfies the probe, so a
    // leak still fails this, within the 5 s instead of hanging.
    let served = retry_until(Duration::from_secs(5), || {
        full_cap_is_reacquirable_concurrently(port, auth)
    });
    assert!(
        served,
        "every slot must be acquirable again once the closed connections release them, \
         so no release path leaks"
    );
}

#[test]
fn idle_flood_beyond_cap_is_refused_not_served() {
    let cfg = HttpConfig {
        // Longer than the poll window below, so holders never get served and
        // released slots cannot muddy the count.
        deadline_secs: Some(30),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    let excess = MAX_CONNECTIONS;
    let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS + excess)
        .map(|_| connect(port))
        .collect();
    // Every connection beyond the cap must be refused with a readable 503,
    // and none of them may ever be served. A refusal persists in the socket
    // buffer, so repeated passes converge regardless of accept order. The
    // budget is sized in passes, not milliseconds: each pass costs about 4.8 s
    // because the in-cap holders never answer and each of their reads waits out
    // its own short socket timeout, so a narrow budget would leave only two
    // passes of headroom and halve that margin on a loaded host.
    let mut refused = vec![false; idle.len()];
    let end = Instant::now() + Duration::from_secs(60);
    while refused.iter().filter(|r| **r).count() < excess && Instant::now() < end {
        for (i, stream) in idle.iter().enumerate() {
            // Skip connections that already answered; re-reading them would
            // spend another socket timeout each pass for no new information.
            if refused[i] {
                continue;
            }
            let mut reader = stream.try_clone().expect("clone for reader");
            let _ = reader.set_read_timeout(Some(Duration::from_millis(150)));
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            if let Some((status, body)) = parse_response(&buf) {
                assert_eq!(
                    status, 503,
                    "a connection beyond the cap must be refused, never served"
                );
                assert!(
                    body.contains("connection limit reached (32)"),
                    "the refusal must name the connection cap: {body}"
                );
                refused[i] = true;
            }
        }
    }
    assert_eq!(
        refused.iter().filter(|r| **r).count(),
        excess,
        "every connection beyond the cap must be refused"
    );
    drop(idle);
    // The whole cap, not one slot. This test opens the cap twice over, so a
    // release path that leaks a share of the slots still leaves enough free for
    // a single sequential probe, and a one-slot probe would pass a leak it is
    // meant to catch. The barrier probe makes every slot contend at once, which
    // is the only way one leaking path shows as a failure here. Polled within a
    // budget, because `drop` returns before the server has processed the closes.
    let served = retry_until(Duration::from_secs(5), || {
        full_cap_is_reacquirable_concurrently(port, "")
    });
    assert!(
        served,
        "the whole cap must be acquirable again once the flood connections close, \
         so no release path leaks"
    );
}

#[test]
fn mid_request_disconnect_releases_the_slot() {
    let cfg = HttpConfig {
        // Longer than every window below, so leaked slots cannot recycle
        // their way to a false pass.
        deadline_secs: Some(60),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    // Build the saturating set once. The retry loop below polls with a single
    // probe connection, so a regression cannot keep appending sockets to the
    // holder set while the pool stays saturated.
    let partials = saturate_with_partials(port, MAX_CONNECTIONS + 8);
    let saturated = retry_until(Duration::from_secs(10), || {
        matches!(
            probe_init(port, ""),
            Some((503, body)) if body.contains("connection limit reached (32)")
        )
    });
    assert!(
        saturated,
        "partial-request connections must saturate the pool"
    );
    // Abrupt mid-request disconnects: every slot must come back without the
    // deadline having to expire, and the whole cap must be reacquirable
    // *simultaneously* so a single leaking release path fails. Polled within a
    // budget, because `drop` returns before the server has processed the
    // disconnects.
    drop(partials);
    let served = retry_until(Duration::from_secs(5), || {
        full_cap_is_reacquirable_concurrently(port, "")
    });
    assert!(
        served,
        "every slot must be acquirable again after mid-request disconnects, so no \
         release path leaks"
    );
}
