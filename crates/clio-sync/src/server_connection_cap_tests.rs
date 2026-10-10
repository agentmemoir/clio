// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's 8-connection cap over the real
//! `sync_serve` entry point on real TCP: the readable 503 above the cap, that
//! the refusal lands only while every slot is genuinely held, and that the
//! **whole** cap comes back after idle connections close or mid-request
//! connections disconnect.
//!
//! ## Owns
//! - The connection-cap refusal and the whole-cap slot-release proofs
//!
//! ## Does not own
//! - The whole-cap probe helpers this module drives
//!   (`server_conn_tests`), which are shared with the stalled-connection test
//! - The byte caps and the request deadline (`server_conn_tests`)
//! - Pool accounting unit coverage (`conn_pool_tests`)
//! - Route semantics (`server_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the connection cap or its release paths change; they
//! never change them.
//!
//! Keep this module focused on connection-cap behavior. Tests assert outcomes
//! and generous bounds, never precise durations.

use super::server_tests::{exchange_until, raw_exchange, serve, status_request};
use crate::conn_pool::MAX_CONNECTIONS;
use crate::server_conn::server_conn_tests::{
    CAP_RELEASE_BUDGET, full_cap_is_reacquirable_concurrently, park_accepted_connections,
    retry_until,
};
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[test]
fn connection_cap_refuses_the_ninth_and_releases_slots_on_close() {
    let server = serve(&["b1"], Some("t1"), false, None, Some(15)).0;
    let addr = server.addr().to_owned();
    let stop = Arc::new(AtomicBool::new(false));
    let mut drips = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut staller = TcpStream::connect(&addr).expect("staller connect");
        staller.write_all(b"POST /sync/p").expect("partial head");
        let stop_flag = Arc::clone(&stop);
        drips.push(thread::spawn(move || {
            // One byte every 250 ms: far under the 5 s read slice, so each
            // byte renews the read wait; only the deadline ends this.
            while !stop_flag.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(250));
                if staller.write_all(b"P").is_err() {
                    break;
                }
            }
        }));
    }
    // The ninth connection is refused with a readable 503 naming the cap.
    let (_, text) = exchange_until(&addr, &status_request(&addr), 503, 10);
    assert!(
        text.contains("connection limit reached (8)"),
        "must name the connection cap: {text}"
    );
    // Close the stallers: EOF releases every slot through the Drop guard. The
    // join below proves every client socket is closed, but not that the server
    // has processed those closes — its connection tasks still have to be
    // scheduled, read the EOF, and unwind. The refill therefore asks within a
    // budget instead of once, so it measures the release path rather than how
    // quickly this thread got scheduled after the join.
    stop.store(true, Ordering::SeqCst);
    for drip in drips {
        drip.join().expect("drip thread");
    }
    // Hold the whole cap at once: probing one connection at a time would only
    // ever need one free slot, so 7 of 8 leaked slots would still pass.
    assert!(
        retry_until(CAP_RELEASE_BUDGET, || {
            full_cap_is_reacquirable_concurrently(&addr)
        }),
        "every slot must be released on stalled/closed paths, so a single \
         leaking path fails this check"
    );
    server.stop();
}

#[test]
fn the_cap_refuses_only_while_every_slot_is_held() {
    let server = serve(&["b1"], Some("t1"), false, None, Some(15)).0;
    let addr = server.addr().to_owned();
    // The fill, the probe and the drain all run in this thread, so each step
    // happens after the previous one: no sleeps are needed to sequence them.
    let mut held = Vec::new();
    for fill in 0..MAX_CONNECTIONS {
        let mut park = TcpStream::connect(&addr).expect("park connect");
        // A partial head is a request the server is serving, not refusing: it
        // holds the slot until the deadline. The probe before it held one too,
        // so both sides of the cap have been observed by the last iteration.
        park.write_all(b"POST /sync/p").expect("partial head");
        if fill + 1 == MAX_CONNECTIONS {
            // Every slot is now held by a parked request: the next connection
            // must be refused. A refusal names the cap.
            let (status, text) = raw_exchange(&addr, &status_request(&addr));
            assert_eq!(
                status, 503,
                "a connection above the fully held cap must be refused: {text}"
            );
            assert!(
                text.contains("connection limit reached (8)"),
                "must name the connection cap: {text}"
            );
        }
        held.push(park);
    }
    // Draining the parked requests releases every slot. Re-acquire the whole
    // cap at once, not just one free slot: a probe that opens and closes a
    // single connection would still see a 200 with 7 of 8 slots leaked, so the
    // recovery half of this test would be unable to see a partial leak.
    drop(held);
    assert!(
        retry_until(CAP_RELEASE_BUDGET, || {
            full_cap_is_reacquirable_concurrently(&addr)
        }),
        "the server must recover the whole cap once the slots drain, so a \
         single leaking release path fails this check"
    );
    server.stop();
}

#[test]
fn mid_request_disconnect_releases_the_slot() {
    let server = serve(&["b1"], Some("t1"), false, None, None).0;
    let addr = server.addr().to_owned();
    // Park and prove accepted `MAX_CONNECTIONS` mid-request connections, so the
    // whole cap is known to be held and to be held on the serving path.
    let parked = park_accepted_connections(&addr, MAX_CONNECTIONS);
    // Abrupt close of all of them at once: the Drop guard must release every
    // slot, not leak them into a permanent outage. Closing them together (not
    // one at a time) is what puts the whole cap through the disconnect path.
    drop(parked);
    // This server runs with no request deadline, so a slot that never came back
    // would stay gone: the refill below is asked within a budget, and a leak
    // spends it without ever succeeding.
    assert!(
        retry_until(CAP_RELEASE_BUDGET, || {
            full_cap_is_reacquirable_concurrently(&addr)
        }),
        "a disconnected mid-request must release every slot; holding the whole \
         cap at once means one leaked slot fails this check"
    );
    server.stop();
}
