// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests bind selection: an explicit address is honored, the auto
//! scan stays on loopback inside the shared range, a busy port advances, and a
//! non-`AddrInUse` error or an exhausted range fails with a clear error.
//!
//! ## Owns
//! - Behavioral tests for `resolve_listener` and `bind_first_free`
//!
//! ## Does not own
//! - Process-level startup contracts (see `tests/mcp_http_bind_harness.rs`)
//! - The occupancy probe for `clio status` (see `port_scan_tests`)
//!
//! ## Boundary
//!
//! Tests here break when bind selection or the exhausted-range error changes;
//! they never change it.
//!
//! Keep this module focused on the bind helper.

use super::*;

fn localhost() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[test]
fn explicit_address_is_honored() {
    // A just-dropped probe port can be taken by a parallel test thread before
    // the explicit rebind, so retry the probe-and-bind until it sticks.
    for _ in 0..100 {
        let probe = TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let addr = probe.local_addr().expect("probe addr");
        drop(probe);
        match resolve_listener(Some(&addr.to_string())) {
            Ok(listener) => {
                assert_eq!(listener.local_addr().expect("addr"), addr);
                return;
            }
            Err(e) if e.kind() != io::ErrorKind::AddrInUse => panic!("bind explicit: {e}"),
            Err(_) => {}
        }
    }
    panic!("no explicit port could be rebound in 100 attempts");
}

#[test]
fn invalid_explicit_address_errors() {
    let err = resolve_listener(Some("999.1.1.1:1")).expect_err("invalid address");
    assert_ne!(err.kind(), io::ErrorKind::AddrInUse);
}

#[test]
fn auto_scan_binds_loopback_in_range() {
    let listener = resolve_listener(None).expect("auto bind");
    let addr = listener.local_addr().expect("addr");
    assert!(addr.ip().is_loopback(), "auto bind must stay on loopback");
    assert!(
        (FIRST_PORT..=LAST_PORT).contains(&addr.port()),
        "auto bind left the shared range: {addr}"
    );
}

#[test]
fn bare_bind_matches_auto_scan() {
    // `parse_flags` reports a value-less `--bind` as the string "true".
    let listener = resolve_listener(Some("true")).expect("bare bind");
    let addr = listener.local_addr().expect("addr");
    assert!(addr.ip().is_loopback());
    assert!((FIRST_PORT..=LAST_PORT).contains(&addr.port()), "{addr}");
}

#[test]
fn occupied_port_advances_to_next_free() {
    // `occupied_with_free_next` can only *prove* the successor is free by
    // binding it and dropping the probe, so the port is unowned for a moment
    // before this bind. Another process can take it in that window, and the
    // range then reads as fully occupied. The window cannot be closed: holding
    // the probe would make the successor occupied and defeat the test. So
    // re-probe and re-bind until a probe survives long enough to be claimed,
    // which is the same mitigation `explicit_address_is_honored` uses. Every
    // attempt still asserts the real behaviour: the scan advances past a busy
    // port rather than failing.
    for _ in 0..100 {
        let (held, busy, free) = occupied_with_free_next();
        match bind_first_free(localhost(), busy..=free) {
            Ok(listener) => {
                assert_eq!(listener.local_addr().expect("addr").port(), free);
                drop(held);
                return;
            }
            Err(e) if e.kind() != io::ErrorKind::AddrInUse => panic!("bind next free: {e}"),
            // The probed successor was taken before we reached it. Probe again.
            Err(_) => {}
        }
    }
    panic!("no probed successor port survived to be bound in 100 attempts");
}

#[test]
fn fully_occupied_range_errors_with_remediation() {
    let held = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = held.local_addr().expect("addr").port();
    let err = bind_first_free(localhost(), port..=port).expect_err("range exhausted");
    assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("{port}-{port}")),
        "range missing: {msg}"
    );
    assert!(
        msg.contains("--bind HOST:PORT"),
        "remediation missing: {msg}"
    );
}

#[test]
fn non_addr_in_use_error_stops_the_scan() {
    // 192.0.2.0/24 (TEST-NET-1) is not assigned to this host, so binding it
    // fails with an address-not-available error instead of advancing.
    let err = bind_first_free(IpAddr::from([192, 0, 2, 1]), 1..=2).expect_err("unowned address");
    assert_ne!(err.kind(), io::ErrorKind::AddrInUse);
}

/// Find an occupied port whose immediate successor is free, holding the
/// occupied listener so the scan must advance past it.
fn occupied_with_free_next() -> (TcpListener, u16, u16) {
    for _ in 0..100 {
        let held = TcpListener::bind("127.0.0.1:0").expect("bind");
        let busy = held.local_addr().expect("addr").port();
        if busy == u16::MAX {
            continue;
        }
        let successor_free = match TcpListener::bind(("127.0.0.1", busy + 1)) {
            Ok(probe) => {
                drop(probe);
                true
            }
            Err(_) => false,
        };
        if successor_free {
            return (held, busy, busy + 1);
        }
    }
    panic!("no occupied port with a free successor found");
}
