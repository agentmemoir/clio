// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one request guard that is not about
//! credentials: Origin validation, which protects the Streamable HTTP transport
//! against DNS rebinding.
//!
//! ## Owns
//! - `check_origin` and `origin_host_allowed` (DNS-rebinding protection)
//! - The parse of an `Origin` value into the host it names
//!
//! ## Does not own
//! - Credential resolution and the 401 answer (see `http_auth`)
//! - Request framing and the response writer (see `http` and `http_read`)
//! - The principal rule a resolved credential then applies (see `principal`)
//! - Session state: the transport is stateless, so no request carries one
//!
//! ## Boundary
//!
//! Changes to Origin policy should normally require changes here. Changes to
//! authentication should not require changes here.
//!
//! Keep this module focused on the origin guard.

use crate::http::HttpConfig;
use crate::http_read::HttpRequest;

/// Origin refusal (`403`) or `None` when the request may proceed.
///
/// The check runs before authentication, so a request from an unapproved Origin
/// never reaches the credential comparison at all.
pub(crate) fn check_origin(cfg: &HttpConfig, req: &HttpRequest) -> Option<u16> {
    let origin = req.get("origin")?;
    if origin_host_allowed(origin, &cfg.allowed_origins) {
        return None;
    }
    Some(403)
}

/// Origin host must be loopback or explicitly allowlisted.
#[must_use]
pub(crate) fn origin_host_allowed(origin: &str, extra: &[String]) -> bool {
    let Some(host) = origin_host(origin) else {
        return false;
    };
    host == "localhost" || host == "127.0.0.1" || host == "[::1]" || extra.iter().any(|e| e == host)
}

/// The host of an Origin value, brackets kept for an IPv6 literal.
///
/// A bracketed address may be followed only by an optional `:port`; anything
/// else after the closing bracket is malformed and refused, so a value such as
/// `[::1].evil` cannot be read as `[::1]`. The port is cut at the colon that
/// follows the closing bracket, so `[::1]:8080` keeps its brackets and compares
/// against the `[::1]` arm above. Splitting on every colon instead would leave
/// `[`, an address no branch of the comparison names.
fn origin_host(origin: &str) -> Option<&str> {
    let authority = origin.split("://").nth(1).unwrap_or(origin);
    let authority = authority.split('/').next().unwrap_or(authority);
    if let Some(after_open) = authority.strip_prefix('[') {
        let end = after_open.find(']')?;
        let rest = &after_open[end + 1..];
        let port_ok = rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
        return port_ok.then(|| &authority[..end + 2]);
    }
    let host = authority.split(':').next().unwrap_or("");
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
#[path = "http_guard_tests.rs"]
mod http_guard_tests;
