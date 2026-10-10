// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two request guards that are not about
//! credentials: Origin validation, and the optional `MCP-Session-Id` session
//! lifecycle the Streamable HTTP transport offers.
//!
//! ## Owns
//! - `HttpShared`: the per-server session registry (id → the identity of the
//!   principal that minted it) and its id counter
//! - `check_origin` and `origin_host_allowed` (DNS-rebinding protection)
//! - `handle_delete` (session termination) and `mint_session`
//!
//! ## Does not own
//! - Credential resolution and the 401 answer (see `http_auth`)
//! - Request framing and the response writer (see `http` and `http_read`)
//! - The principal rule a resolved credential then applies (see `principal`)
//!
//! ## Boundary
//!
//! Changes to Origin policy or session lifecycle should normally require changes
//! here. Changes to authentication should not require changes here.
//!
//! Keep this module focused on the origin and session guards.
//!
//! # Session binding
//!
//! On a credential-bound server a session is minted for the principal that ran
//! `initialize`, and it stays live only for that same principal. A session id
//! presented by anyone else is answered exactly like an unknown id, so a stolen
//! id buys nothing without the credential that minted it. The identity is not a
//! secret and is never logged; it is only compared. The identity is the
//! principal's record label, which the store keeps unique, so two different
//! credentials can never compare equal and cannot reach each other's sessions.
//! On an unbound server every session is stored as unowned and the lifecycle is
//! unchanged.

use crate::http::HttpConfig;
use crate::http_read::HttpRequest;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Mutex;

/// Per-server mutable transport state.
#[derive(Debug, Default)]
pub(crate) struct HttpShared {
    /// Session id → the identity of the minting principal; `None` for a session
    /// minted on an unbound server.
    sessions: Mutex<HashMap<String, Option<String>>>,
    session_counter: std::sync::atomic::AtomicU64,
}

impl HttpShared {
    /// Whether `id` is a live session for the requester with identity
    /// `identity`.
    ///
    /// A session is live only when it exists AND its stored identity equals the
    /// requester's: `None` matches `None`, and two equal identities match. This
    /// is the one predicate the session gate checks, so a bound server refuses a
    /// cross-principal presentation exactly as it refuses an unknown id.
    pub(crate) fn is_live(&self, id: &str, identity: Option<&str>) -> bool {
        self.sessions.lock().is_ok_and(|m| {
            m.get(id)
                .is_some_and(|stored| stored.as_deref() == identity)
        })
    }
}

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
/// The port is cut at the colon that follows the closing bracket, so a
/// bracketed address such as `[::1]:8080` keeps its brackets and compares
/// against the `[::1]` arm above. Splitting on every colon instead would leave
/// `[`, an address no branch of the comparison names.
fn origin_host(origin: &str) -> Option<&str> {
    let authority = origin.split("://").nth(1).unwrap_or(origin);
    let authority = authority.split('/').next().unwrap_or(authority);
    if let Some(end) = authority.strip_prefix('[').and_then(|rest| rest.find(']')) {
        return Some(&authority[..end + 2]);
    }
    let host = authority.split(':').next().unwrap_or("");
    (!host.is_empty()).then_some(host)
}

/// `DELETE` with an unknown, absent, or foreign session id; the owner's
/// `DELETE` removes it.
pub(crate) fn handle_delete(
    shared: &HttpShared,
    req: &HttpRequest,
    identity: Option<&str>,
) -> (u16, String, Vec<(String, String)>) {
    let Some(id) = req.get("mcp-session-id") else {
        return (400, String::new(), vec![]);
    };
    // Only the minting principal may terminate a session; any other identity is
    // answered like an unknown id and leaves the session in place.
    let removed = shared.sessions.lock().is_ok_and(|mut m| {
        m.get(id)
            .is_some_and(|stored| stored.as_deref() == identity)
            && m.remove(id).is_some()
    });
    if removed {
        (200, String::new(), vec![])
    } else {
        (404, String::new(), vec![])
    }
}

/// Mint a fresh opaque 128-bit session id and register it under `identity`, the
/// requester's principal identity (`None` on an unbound server).
pub(crate) fn mint_session(shared: &HttpShared, identity: Option<&str>) -> String {
    use std::hash::{BuildHasher, Hasher};
    let counter = shared
        .session_counter
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // `RandomState` carries OS-seeded keys; two keyed draws plus the monotonic
    // counter give an unguessable id rather than a predictable counter+clock.
    let seed = std::collections::hash_map::RandomState::new();
    let mut id = String::from("sess-");
    for draw in [counter, 0] {
        let mut hasher = seed.build_hasher();
        hasher.write_u64(draw);
        let _ = write!(id, "{:016x}", hasher.finish());
    }
    if let Ok(mut sessions) = shared.sessions.lock() {
        sessions.insert(id.clone(), identity.map(str::to_owned));
    }
    id
}

#[cfg(test)]
#[path = "http_guard_tests.rs"]
mod http_guard_tests;
