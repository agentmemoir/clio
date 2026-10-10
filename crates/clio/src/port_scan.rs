// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bounded loopback TCP port scan that
//! reports which `clio mcp http` listener ports are occupied.
//!
//! ## Owns
//! - The shared `34300-34309` listener range constants
//! - The short-timeout connect probe over a loopback address and port range
//!
//! ## Does not own
//! - Binding or owning a listener (the HTTP transport does that)
//! - What the caller does with the discovered ports (see `status_cli`)
//!
//! ## Boundary
//!
//! Changes to the scanned range or probe timeout should normally require
//! changes here. Changes to how listeners are reported should not.
//!
//! Keep this module focused on the bounded loopback listener probe.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::ops::RangeInclusive;
use std::time::Duration;

/// First port of the shared HTTP-listener range (`34300-34309`).
pub(crate) const FIRST_PORT: u16 = 34300;

/// Last port of the shared HTTP-listener range (`34300-34309`).
pub(crate) const LAST_PORT: u16 = 34309;

/// Per-port connect timeout; ten ports bound the whole scan to well under two
/// seconds even when every port is filtered rather than refused.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);

/// The `127.0.0.1:34300-34309` ports that currently accept a connection.
///
/// Bounded: each port gets at most [`CONNECT_TIMEOUT`] and a refused connection
/// is a normal "no listener" result, never an error.
#[must_use]
pub(crate) fn listening_ports() -> Vec<u16> {
    scan(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        FIRST_PORT..=LAST_PORT,
        CONNECT_TIMEOUT,
    )
}

/// Probe each port in `ports` on `addr`, returning the ones that accept a
/// connection. Sorted ascending (range order); a refused/timed-out port is
/// simply absent.
pub(crate) fn scan(addr: IpAddr, ports: RangeInclusive<u16>, timeout: Duration) -> Vec<u16> {
    ports
        .filter(|port| TcpStream::connect_timeout(&SocketAddr::new(addr, *port), timeout).is_ok())
        .collect()
}

#[cfg(test)]
#[path = "port_scan_tests.rs"]
mod tests;
