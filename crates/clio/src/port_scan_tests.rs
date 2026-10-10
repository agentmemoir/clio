// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the bounded loopback port scan: it reports an occupied
//! port, omits a free port, and stays inside the shared range.
//!
//! ## Owns
//! - Occupied/free port detection and range-bound tests for `port_scan`
//!
//! ## Does not own
//! - The `clio status` command surface (see `status_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the scan contract or range changes; they never change it.
//!
//! Keep this module focused on the port-scan helper.

use super::*;
use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;

const FAST: Duration = Duration::from_millis(150);

fn localhost() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[test]
fn occupied_port_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    let found = scan(localhost(), port..=port, FAST);
    assert_eq!(found, vec![port], "bound port must be reported");
}

/// A listener inside the shared range must be reported by the range wrapper.
///
/// The port claimed is the range's **last**, not its first (#78). Both
/// `auto_scan_binds_loopback_in_range` and `bare_bind_matches_auto_scan` reach
/// the range through `bind_first_free`, which walks it ascending from
/// `FIRST_PORT` and returns the first free port — so with only those two tests
/// on the machine they take `FIRST_PORT` and never look at `LAST_PORT`. This
/// test used to bind `FIRST_PORT`, which made it race them for the same socket:
/// when a sibling held the port the bind failed, and the fallback then asserted
/// against a listener the sibling had already dropped, so the failure read
/// "port 34300 is unavailable yet not reported".
///
/// Holding `LAST_PORT` removes the contention rather than retrying through it,
/// which also keeps the fallback branch honest. If the bind still fails, the
/// owner is something outside this test binary — a real service or the host —
/// and that listener does not vanish between the failed bind and the scan.
#[test]
fn listener_on_contract_port_is_reported() {
    let listener = match TcpListener::bind(("127.0.0.1", LAST_PORT)) {
        Ok(listener) => listener,
        Err(e) => {
            assert!(
                listening_ports().contains(&LAST_PORT),
                "port {LAST_PORT} is unavailable ({e}) yet not reported"
            );
            return;
        }
    };
    assert!(
        listening_ports().contains(&LAST_PORT),
        "a listener bound on {LAST_PORT} must be reported"
    );
    drop(listener);
}

#[test]
fn free_port_is_omitted() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    assert!(
        !scan(localhost(), port..=port, FAST).contains(&port),
        "closed port must not be reported"
    );
}

#[test]
fn range_is_sorted_and_bounded() {
    let ports = listening_ports();
    assert!(
        ports.iter().all(|p| (FIRST_PORT..=LAST_PORT).contains(p)),
        "only the shared range is scanned: {ports:?}"
    );
    let mut sorted = ports.clone();
    sorted.sort_unstable();
    assert_eq!(ports, sorted, "scan result is ascending");
}

#[test]
fn shared_range_matches_the_contract() {
    assert_eq!(FIRST_PORT, 34300);
    assert_eq!(LAST_PORT, 34309);
}
