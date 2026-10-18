// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for describing a Postgres test target without
//! ever exposing its credentials: one reachability probe over the driver's own
//! parse of the URL, and one printable form that carries scheme, host, and port
//! only.
//!
//! ## Owns
//! - `postgres_reachable`: whether a server answers at a URL's host and port
//! - `redacted_endpoint`: the credential-safe printable form of a URL
//!
//! ## Does not own
//! - The skip or fail-closed decision itself (`pg_skip`)
//! - The connection machine or TLS connector (`pg_tls`)
//! - Whether a test probes at all (each harness's own gate)
//!
//! ## Boundary
//!
//! Changes to how a test target is probed or described should normally require
//! changes here. Changes to the skip policy should not.
//!
//! Test-only: the seam is compiled only under `cfg(test)` or the
//! `test-support` feature, which a dependent crate turns on from
//! `[dev-dependencies]`; no shipped build carries it.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The connect budget for one probe: enough for a local or SSH-forwarded
/// server, short enough that an unreachable host does not stall the suite.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// True when a Postgres server answers at `url`'s host and port within the
/// probe budget.
///
/// The host and port come from the driver's own parse of the string, so the
/// answer agrees with [`redacted_endpoint`] and with what the harness would
/// dial. A string the driver refuses, or one that names no host, is not
/// reachable.
#[must_use]
pub fn postgres_reachable(url: &str) -> bool {
    let Ok(connection) = crate::parse_connection(url) else {
        return false;
    };
    let host = connection.primary_host();
    if host.is_empty() {
        return false;
    }
    let Ok(addresses) = (host, connection.primary_port()).to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok())
}

/// `url` as `scheme://host:port`, with any userinfo removed.
///
/// The parse is the driver's own, so a `/` or `?` inside a password cannot
/// truncate the userinfo and print `user:password-prefix`. A string the driver
/// refuses, or whose parsed host is not a plain host, becomes the fixed
/// placeholder `<redacted>`: a URL that cannot be described safely is never
/// echoed, so no credential reaches the log. The scheme is read from the
/// string's prefix, which is the part before any userinfo.
#[must_use]
pub fn redacted_endpoint(url: &str) -> String {
    match crate::parse_connection(url) {
        Ok(connection) if is_plain_host(connection.primary_host()) => format!(
            "{}://{}:{}",
            scheme_of(url),
            connection.primary_host(),
            connection.primary_port()
        ),
        _ => "<redacted>".to_owned(),
    }
}

/// The scheme of `url`, defaulting to `postgres` for anything else.
fn scheme_of(url: &str) -> &'static str {
    if url.starts_with("postgresql://") {
        "postgresql"
    } else {
        "postgres"
    }
}

/// Whether `host` is a plain DNS name or IPv4 literal, with no separator a
/// credential could hide in. An IPv6 or unix-socket host is not plain, so it is
/// redacted rather than risk printing a fragment of one.
fn is_plain_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::{postgres_reachable, redacted_endpoint};

    /// A normal URL prints only scheme, host, and port: the credential readers
    /// in a harness can compare against the userinfo and see it is absent.
    #[test]
    fn a_redacted_endpoint_names_only_the_scheme_host_and_port() {
        let url = "postgres://clio:secret@127.0.0.1:34310/clio";
        assert_eq!(redacted_endpoint(url), "postgres://127.0.0.1:34310");
    }

    /// Regression cover: a `/` or `?` inside the password used to truncate the
    /// authority before the userinfo, so the message printed `user:pw-prefix`.
    /// The driver's parser reads userinfo up to the `@` wherever it sits, so no
    /// credential can reach the printable form.
    #[test]
    fn a_redacted_endpoint_never_prints_a_password_with_a_separator() {
        for url in [
            "postgres://cust0mer:pa/ss@127.0.0.1:34310/clio",
            "postgres://cust0mer:pa?ss@127.0.0.1:34310/clio",
            "postgres://cust0mer:pa/ss?opt=1@127.0.0.1:34310/clio",
        ] {
            let shown = redacted_endpoint(url);
            assert!(!shown.contains("cust0mer"), "user leaked: {shown}");
            assert!(!shown.contains("pa/ss"), "password leaked: {shown}");
            assert!(!shown.contains("pa?ss"), "password leaked: {shown}");
            assert!(!shown.contains('@'), "userinfo leaked: {shown}");
        }
    }

    /// A string the driver refuses is not described at all.
    #[test]
    fn a_redacted_endpoint_refuses_an_unparseable_string() {
        assert_eq!(redacted_endpoint("not a url"), "<redacted>");
        assert_eq!(redacted_endpoint("postgres://"), "<redacted>");
    }

    /// The probe agrees with the redaction: a string the driver refuses is not
    /// reachable, and a bad string cannot make the probe dial a credential.
    #[test]
    fn the_probe_refuses_an_unparseable_string() {
        assert!(!postgres_reachable("not a url"));
        assert!(!postgres_reachable("postgres://"));
    }
}
