// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the Origin guard, the one
//! request guard that is not about credentials.
//!
//! ## Owns
//! - `check_origin` and `origin_host_allowed`: the absent header, the loopback
//!   and allowlisted hosts, and the refusal
//! - The host parse: scheme and port stripping, IPv6 brackets, malformed values
//!
//! ## Does not own
//! - The transport harness those tests drive (`http_test_support_tests`)
//! - Request parsing and the response write (`http_read`, `http`)
//!
//! ## Boundary
//!
//! Changes to what the origin guard decides should normally require changes
//! here. Changes to credential resolution or request framing should not require
//! changes here.
//!
//! Keep this module focused on the guard, called through its own signature with
//! no socket involved.

use super::*;

/// A parsed request carrying `headers`, which is the whole surface the guard
/// reads. The reader lowercases header names before a request reaches a guard,
/// so the names here are lowercase too.
fn request_with(method: &str, headers: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        path: "/mcp".to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: String::new(),
    }
}

/// A config whose only extra allowlisted Origin host is `host`.
fn allowing(host: &str) -> HttpConfig {
    HttpConfig {
        allowed_origins: vec![host.to_owned()],
        ..HttpConfig::default()
    }
}

/// A request that names no Origin at all is not an Origin-bearing request, so
/// the guard has nothing to refuse and the request proceeds.
#[test]
fn a_request_without_an_origin_is_not_refused() {
    assert_eq!(
        check_origin(&HttpConfig::default(), &request_with("POST", &[])),
        None
    );
}

/// Loopback is always allowed and any other host is refused, whatever scheme
/// and port the Origin carries.
#[test]
fn check_origin_allows_loopback_and_refuses_another_host() {
    let cfg = HttpConfig::default();
    for origin in [
        "http://localhost:5173",
        "https://127.0.0.1",
        "http://localhost",
        "https://127.0.0.1:8443",
        "http://[::1]:5173",
    ] {
        assert_eq!(
            check_origin(&cfg, &request_with("POST", &[("origin", origin)])),
            None,
            "loopback must be allowed: {origin}"
        );
    }
    assert_eq!(
        check_origin(
            &cfg,
            &request_with("POST", &[("origin", "http://evil.example.com")])
        ),
        Some(403),
        "another host must be refused"
    );
}

/// A host the operator named is allowed, and the allowlist does not widen past
/// the host it names.
#[test]
fn check_origin_allows_a_configured_host_and_nothing_more() {
    let cfg = allowing("custom-host.example.com");
    assert_eq!(
        check_origin(
            &cfg,
            &request_with("POST", &[("origin", "https://custom-host.example.com")])
        ),
        None,
        "the configured host must be allowed"
    );
    assert_eq!(
        check_origin(
            &cfg,
            &request_with("POST", &[("origin", "https://other-host.example.com")])
        ),
        Some(403),
        "a host the operator did not name must be refused"
    );
}

/// The host policy on its own: the scheme is dropped, the port is cut from the
/// host it follows — after the closing bracket for an IPv6 literal — a bare host
/// with no scheme is compared as written, and everything not loopback or
/// configured fails.
#[test]
fn origin_host_allowed_accepts_loopback_and_configured_hosts_only() {
    let extra = vec![
        "custom-host.example.com".to_owned(),
        "[2001:db8::1]".to_owned(),
    ];
    for origin in [
        "http://localhost:8080",
        "https://127.0.0.1",
        "http://127.0.0.1:9000",
        "http://[::1]:38417",
        "http://[::1]",
        "custom-host.example.com",
        "https://[2001:db8::1]:9000",
    ] {
        assert!(origin_host_allowed(origin, &extra), "must allow {origin}");
    }
    for origin in [
        "http://evil.example.com",
        "https://localhost.evil.example.com",
        "http://[::1].evil",
        "http://[::1]evil",
        "http://[::1]:80@evil.example.com",
        "",
    ] {
        assert!(!origin_host_allowed(origin, &extra), "must refuse {origin}");
    }
}
