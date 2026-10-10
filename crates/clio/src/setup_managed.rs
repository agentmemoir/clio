// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the values a Clio-managed Postgres is given,
//! and for choosing the one of them that is not fixed: the port.
//!
//! ## Owns
//! - The host, user, and database name the managed container always runs under
//! - The port range and the order it is tried in
//! - `pick_managed_port`, which returns the first free port in that order
//!
//! ## Does not own
//! - The question that reports the chosen values (see `setup_flows_db`)
//! - Assembling the connection address (see `setup_plan_db`)
//! - The `.env` file the container reads (see `compose_env`)
//!
//! ## Boundary
//!
//! Changes to which values a managed database gets, or to the port range, should
//! normally require changes here. Changes to a question or to address assembly
//! should not.
//!
//! Keep this module focused on the fixed values and the port choice.
//!
//! # Why the port is the only value that is searched
//!
//! The user, the database name, and the host are written into the container by
//! `compose.yml`, and a value that disagrees with them produces a connection that
//! can never work. Only the port can be wrong through no fault of the operator:
//! another program may already hold the one Clio would prefer. So the port is
//! searched, and everything else is a constant the operator never sees a question
//! about.
//!
//! # Why the port is handed to Compose through the environment
//!
//! `compose.yml` reads `${CLIO_POSTGRES_PORT:-34310}`. Passing the value to the
//! child process is what makes the container and the stored address agree, and
//! it leaves the user's own `.env` untouched — a file this wizard must never
//! overwrite. See `setup_wizard::managed_env`.
//!
//! # Why there is no password
//!
//! The managed database is a single-user local store, so the port binding is
//! the boundary that matters: `compose.yml` publishes it on `127.0.0.1` and asks
//! the image for trust authentication. That is the arrangement PostgreSQL's own
//! documentation describes as reasonable — trust "is seldom reasonable to use
//! for any TCP/IP connections other than those from localhost" — and the caveat
//! it carries is the accepted one: "Local TCP/IP connections are not restricted
//! by file-system permissions", so any process on this machine can still
//! connect. That is the ordinary exposure of a local development database. What
//! must not remain is the exposure off the machine.

use std::net::{Ipv4Addr, TcpListener};

/// The host the managed container is reached on.
///
/// Loopback, not a hostname: the container publishes the port on this machine and
/// nothing else, and `localhost` resolves to an IPv6 address on some hosts where
/// only IPv4 is published.
pub(crate) const MANAGED_HOST: &str = "127.0.0.1";

/// The role Clio creates in the managed database, matching `CLIO_POSTGRES_USER`.
pub(crate) const MANAGED_USER: &str = "clio";

/// The database name Clio creates, matching `CLIO_POSTGRES_DB`.
pub(crate) const MANAGED_DATABASE: &str = "clio";

/// The port Clio tries first.
///
/// One above the Postgres default, inside the range below, and the value
/// `compose.yml` already defaults to, so a run that takes it needs no override.
pub(crate) const PREFERRED_PORT: u16 = 34310;

/// The lowest port the managed database may use.
pub(crate) const LOWEST_PORT: u16 = 34300;

/// The ports in the order they are tried, preferred first.
///
/// Eleven ports, from [`PREFERRED_PORT`] down to [`LOWEST_PORT`]. Wide enough
/// that eleven busy ports on one machine is unlikely, narrow enough that the port
/// is recognisably Clio's and a script can find it without being told.
#[must_use]
pub(crate) fn ports_in_preference_order() -> Vec<u16> {
    (LOWEST_PORT..=PREFERRED_PORT).rev().collect()
}

/// The first port in [`ports_in_preference_order`] nothing is listening on.
///
/// Binding is the test: a bind that succeeds means the port is free, and the
/// listener is dropped immediately so the port is unclaimed by the time the
/// container starts. The check has a race with any other process starting at the
/// same moment, which is a window a single-user local install does not close
/// either way; the failure it would produce is a connection refused, not a lost
/// volume.
///
/// # Errors
/// Every port in the range is in use, which is reported with the range rather
/// than with a single port so the operator can free one and know which.
pub(crate) fn pick_managed_port() -> Result<String, String> {
    let tried = ports_in_preference_order();
    for port in &tried {
        if TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok() {
            return Ok(port.to_string());
        }
    }
    Err(format!(
        "Clio cannot start its own Postgres: ports {LOWEST_PORT} to {PREFERRED_PORT} are all in \
         use. Stop whatever is holding one, or point Clio at a Postgres you already have."
    ))
}

#[cfg(test)]
#[path = "setup_managed_tests.rs"]
mod tests;
