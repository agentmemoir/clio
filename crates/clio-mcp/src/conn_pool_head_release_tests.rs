// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves that the header-read deadline returns the connection
//! slots held by unauthenticated partial-head connections, so the whole cap is
//! acquirable again long before the total request deadline expires.
//!
//! ## Owns
//! - The header-deadline release proof, driven through the real
//!   `http::serve_with_listener` on an OS-assigned port
//!
//! ## Does not own
//! - The header budget and the 408 it produces (see `http` and `http_read`)
//! - Slot counting, the `Drop` release, and the cap refusal (see `conn_pool`
//!   and `conn_pool_transport_bounds_tests`)
//!
//! ## Boundary
//!
//! Changes to how the header budget releases a connection slot should normally
//! require changes here. Changes to routing, sessions, or tool behavior should
//! not require changes here.
//!
//! Keep this module focused on the header-deadline release of connection slots.

use super::conn_pool_transport_bounds_tests::{
    full_cap_is_reacquirable_concurrently, parse_response, probe_served, retry_until,
    saturate_with_partials, spawn_server,
};
use crate::conn_pool::MAX_CONNECTIONS;
use crate::http::HttpConfig;
use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Ceiling on the wait for the header budget to expire a parked partial head.
/// The budget under test is 5 s, so this leaves a wide margin above the expiry
/// it waits for and still turns a release that never fires into a bounded
/// failure instead of a hung gate.
const HEAD_EXPIRY_BUDGET: Duration = Duration::from_secs(15);

/// Ceiling on the full-cap reacquisition once the header deadline has fired.
/// Wide enough for a loaded host to schedule 32 contending threads.
const REACQUIRE_BUDGET: Duration = Duration::from_secs(10);

/// Longest single read spent on one holder during the expiry wait. Short, so
/// the pass reaches every holder before the header budget fires.
const EXPIRY_POLL: Duration = Duration::from_millis(50);

/// Wait, bounded, for the header budget to expire a parked partial head.
///
/// The signal is a real 408 on a real socket, not a sleep: the pass returns as
/// soon as one holder answers with it. A holder the cap refused answers at once
/// with its 503, and a holder still inside its budget answers nothing, so the
/// pass repeats until the budget runs out. `true` means the header deadline was
/// observed to fire.
fn await_head_expiry(held: &[TcpStream], budget: Duration) -> bool {
    let end = Instant::now() + budget;
    loop {
        for stream in held {
            let Ok(mut reader) = stream.try_clone() else {
                continue;
            };
            let _ = reader.set_read_timeout(Some(EXPIRY_POLL));
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            if matches!(parse_response(&buf), Some((408, _))) {
                return true;
            }
        }
        if Instant::now() >= end {
            return false;
        }
    }
}

#[test]
fn header_deadline_releases_partial_head_slots() {
    let cfg = HttpConfig {
        // Six times shorter than the total deadline below, so the header budget
        // is the only bound that can free a slot: if the total deadline were
        // the binding bound, a release would prove nothing about the header
        // budget. Long enough that saturating all 32 slots inside one budget is
        // not a scheduling race.
        deadline_secs: Some(30),
        head_deadline_secs: Some(5),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    // Over-provisioned the way the idle proof is: the accept loop refuses above
    // the cap rather than queueing, so a few of these may be refused while the
    // remaining ones hold all 32 slots.
    let held = saturate_with_partials(port, MAX_CONNECTIONS + 8);
    // The cap is genuinely held while the partial heads live. A probe that reads
    // the refusal naming the cap can only have been refused after every slot was
    // taken, so this is the receipt that the partial heads own the whole pool.
    let saturated = retry_until(Duration::from_secs(10), || {
        matches!(
            probe_served(port, ""),
            Some((503, body)) if body.contains("connection limit reached (32)")
        )
    });
    assert!(
        saturated,
        "unauthenticated partial heads must hold the whole cap before the header \
         deadline can be shown to release it"
    );
    // Bounded wait on the header budget itself, then the cap check. A release
    // path that holds the slot for the total deadline instead leaves the cap
    // full, and the reacquisition below fails within its budget.
    let expired = await_head_expiry(&held, HEAD_EXPIRY_BUDGET);
    let served = retry_until(REACQUIRE_BUDGET, || {
        full_cap_is_reacquirable_concurrently(port, "")
    });
    assert!(
        served,
        "the whole cap must be acquirable once the header deadline expires on \
         unauthenticated partial heads; header 408 observed: {expired}"
    );
    drop(held);
}
