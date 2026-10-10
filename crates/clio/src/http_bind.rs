// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning the `clio mcp http` `--bind` flag into
//! a bound `TcpListener`: an explicit `HOST:PORT` is honored, while an absent or
//! value-less flag scans the shared `127.0.0.1` listener range for the first
//! free port.
//!
//! ## Owns
//! - Distinguishing an automatic bind from an explicit `HOST:PORT`
//! - The sequential first-free-port scan over the shared `port_scan` range
//! - The "port in use" versus other-bind-error distinction
//!
//! ## Does not own
//! - Serving the listener or the non-loopback auth rule (see `clio_mcp::http`)
//! - Occupancy probing for `clio status` (see `port_scan`)
//!
//! ## Boundary
//!
//! Changes to bind selection or the exhausted-range error should normally
//! require changes here. Changes to how a listener is served should not.
//!
//! Keep this module focused on selecting a listener to bind.

use crate::port_scan::{FIRST_PORT, LAST_PORT};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::ops::RangeInclusive;

/// Resolve the `--bind` flag value into a bound listener.
///
/// `None` (no flag) and a bare `--bind` both scan `127.0.0.1:34300-34309` for
/// the first free port; `parse_flags` reports a value-less flag as the string
/// `"true"`. Any other value is bound directly as an explicit `HOST:PORT`.
///
/// # Errors
/// An explicit address that cannot bind, or an auto range with no free port.
pub(crate) fn resolve_listener(bind_flag: Option<&str>) -> io::Result<TcpListener> {
    match bind_flag {
        None | Some("true") => {
            bind_first_free(IpAddr::V4(Ipv4Addr::LOCALHOST), FIRST_PORT..=LAST_PORT)
        }
        Some(addr) => TcpListener::bind(addr),
    }
}

/// Bind the first free port in `ports` on `addr`.
///
/// A port already in use advances to the next candidate. Any other bind error
/// (for example an address this host does not own) stops the scan so a real
/// problem is never mistaken for a busy port. When every candidate is in use
/// the returned error names the range and the remediation.
///
/// # Errors
/// The first non-`AddrInUse` bind error, or `AddrInUse` once the range is
/// exhausted.
pub(crate) fn bind_first_free(addr: IpAddr, ports: RangeInclusive<u16>) -> io::Result<TcpListener> {
    let (first, last) = (*ports.start(), *ports.end());
    for port in ports {
        match TcpListener::bind(SocketAddr::new(addr, port)) {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!(
            "no free port in {first}-{last} on {addr}; stop the other listener or pass --bind HOST:PORT"
        ),
    ))
}

#[cfg(test)]
#[path = "http_bind_tests.rs"]
mod tests;
