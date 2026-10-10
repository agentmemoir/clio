// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the two request guards
//! that are not about credentials: Origin validation, and the optional
//! `MCP-Session-Id` session lifecycle.
//!
//! ## Owns
//! - `HttpShared::is_live` over a bare registry: the owner reuses, another
//!   identity is refused, an unowned session stays unowned, and an unknown id
//!   is live for nobody
//! - `check_origin` and `origin_host_allowed`: the absent header, the loopback
//!   and allowlisted hosts, and the refusal
//! - `handle_delete`: no session id, an unknown id, the owner's delete, and a
//!   foreign delete that changes nothing
//! - `mint_session`: the shape of a minted id and its distinctness
//!
//! ## Does not own
//! - The same guards over a live bound server (`http_binding_tests`)
//! - The socket harness those tests drive (`http_test_support_tests`)
//! - Request parsing and the response write (`http_read`, `http`)
//!
//! ## Boundary
//!
//! Changes to what the origin or session guards decide should normally require
//! changes here. Changes to credential resolution or request framing should not
//! require changes here.
//!
//! Keep this module focused on the guards, called through their own signatures
//! with no socket involved.

use super::*;

/// A parsed request carrying `headers`, which is the whole surface either guard
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

/// The binding predicate at the registry layer, with no socket involved.
///
/// The identity a session binds to is its owner's record label, so a requester
/// matches only the entry minted under its own identity.
#[test]
fn a_minted_session_is_live_only_for_its_own_identity() {
    let shared = HttpShared::default();
    let owned = mint_session(&shared, Some("ci"));
    let free = mint_session(&shared, None);

    assert!(shared.is_live(&owned, Some("ci")), "the owner may reuse it");
    assert!(
        !shared.is_live(&owned, Some("bot")),
        "another identity must be refused"
    );
    assert!(
        !shared.is_live(&owned, None),
        "an unowned presentation must be refused"
    );
    assert!(
        shared.is_live(&free, None),
        "an unowned session stays unowned"
    );
    assert!(
        !shared.is_live(&free, Some("ci")),
        "an identified requester must not claim an unowned session"
    );
    assert!(
        !shared.is_live("sess-unknown", Some("ci")),
        "an unknown id is live for nobody"
    );
}

/// A minted id is a fixed prefix followed by two 64-bit draws in hex, so a
/// client cannot derive one id from another. The draws themselves are seeded
/// from the OS, so the value is not pinned.
#[test]
fn a_minted_session_id_is_opaque_hex_behind_a_prefix() {
    let shared = HttpShared::default();
    let id = mint_session(&shared, Some("ci"));

    let Some(draws) = id.strip_prefix("sess-") else {
        panic!("a minted id carries the session prefix: {id}");
    };
    assert_eq!(draws.len(), 32, "two 64-bit draws in hex: {id}");
    assert!(
        draws.chars().all(|c| c.is_ascii_hexdigit()),
        "the draws are hex: {id}"
    );
}

/// Two mints are two entries. The counter moves on every mint, so no id repeats
/// however the draws fall; the ids are sorted before comparison so the check
/// never depends on the order the registry happens to return.
#[test]
fn every_minted_session_id_is_distinct() {
    let shared = HttpShared::default();
    let ids: Vec<String> = (0..64).map(|_| mint_session(&shared, None)).collect();

    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "64 mints, 64 distinct ids");
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
            &request_with("POST", &[("origin", "http://evil.example")])
        ),
        Some(403),
        "another host must be refused"
    );
}

/// A host the operator named is allowed, and the allowlist does not widen past
/// the host it names.
#[test]
fn check_origin_allows_a_configured_host_and_nothing_more() {
    let cfg = allowing("custom-host");
    assert_eq!(
        check_origin(
            &cfg,
            &request_with("POST", &[("origin", "https://custom-host")])
        ),
        None,
        "the configured host must be allowed"
    );
    assert_eq!(
        check_origin(
            &cfg,
            &request_with("POST", &[("origin", "https://other-host")])
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
    let extra = vec!["custom-host".to_owned(), "[2001:db8::1]".to_owned()];
    for origin in [
        "http://localhost:8080",
        "https://127.0.0.1",
        "http://127.0.0.1:9000",
        "http://[::1]:38417",
        "http://[::1]",
        "custom-host",
        "https://[2001:db8::1]:9000",
    ] {
        assert!(origin_host_allowed(origin, &extra), "must allow {origin}");
    }
    for origin in ["http://evil.example", "https://localhost.evil.example", ""] {
        assert!(!origin_host_allowed(origin, &extra), "must refuse {origin}");
    }
}

/// A `DELETE` carrying no session id is a malformed request, answered before
/// any identity is compared, with an empty body and no headers.
#[test]
fn a_delete_without_a_session_id_is_a_bad_request() {
    let shared = HttpShared::default();
    let (status, body, headers) = handle_delete(&shared, &request_with("DELETE", &[]), Some("ci"));

    assert_eq!(status, 400, "a delete must name a session");
    assert!(body.is_empty(), "the refusal advertises no body");
    assert!(headers.is_empty(), "and no header");
}

/// A session id nobody minted is refused, and refused as an unknown id.
#[test]
fn a_delete_of_an_unknown_session_id_is_404() {
    let shared = HttpShared::default();
    let (status, _, _) = handle_delete(
        &shared,
        &request_with("DELETE", &[("mcp-session-id", "sess-absent")]),
        Some("ci"),
    );

    assert_eq!(status, 404);
}

/// The minting identity may terminate its session, and the session is gone
/// afterwards.
#[test]
fn the_owner_can_delete_its_own_session() {
    let shared = HttpShared::default();
    let id = mint_session(&shared, Some("ci"));
    let (status, body, headers) = handle_delete(
        &shared,
        &request_with("DELETE", &[("mcp-session-id", &id)]),
        Some("ci"),
    );

    assert_eq!(status, 200, "the owner's delete is accepted");
    assert!(body.is_empty(), "with no body");
    assert!(headers.is_empty(), "and no header");
    assert!(!shared.is_live(&id, Some("ci")), "and it is gone");
}

/// A delete from another identity is answered exactly like an unknown id and
/// removes nothing: the owner's session is still live afterwards.
#[test]
fn a_foreign_delete_leaves_the_session_live() {
    let shared = HttpShared::default();
    let id = mint_session(&shared, Some("ci"));
    let (status, _, _) = handle_delete(
        &shared,
        &request_with("DELETE", &[("mcp-session-id", &id)]),
        Some("bot"),
    );

    assert_eq!(status, 404, "a foreign delete must be refused");
    assert!(shared.is_live(&id, Some("ci")), "and it removed nothing");
}
