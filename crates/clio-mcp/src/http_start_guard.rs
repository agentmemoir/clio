// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the startup admission decision of an MCP HTTP
//! listener: whether a bind may serve at all, given its credential source and
//! the operator's explicit opt-outs.
//!
//! ## Owns
//! - `bind_refusal`: the credential requirement and the non-loopback bind
//!   refusal, decided once before the accept loop starts
//! - `BindRefusal`: why a bind is refused, including the rule that a
//!   non-loopback bind with no credential needs both opt-outs
//! - `has_credential_source`: whether a configuration carries a credential at
//!   all, the notion `bind_refusal` and `serves_untrusted_network` read
//! - `serves_untrusted_network`: the listener's reachability, the fact a served
//!   bind passes to its handler
//! - `is_proxied_or_remote`: the caller's origin, decided per request
//!
//! ## Does not own
//! - Request authentication, which still governs every request whatever the
//!   start-time opt-outs (see `http_auth`)
//! - Binding the socket and reporting the refusal to the operator (see `http`)
//! - Origin validation and session lifecycle (see `http_guard`)
//!
//! ## Boundary
//!
//! Changes to what a bind requires before it serves, and to what makes a caller
//! local, should normally require changes here. Changes to how one request is
//! authorized should not.
//!
//! Keep this module focused on the start-time and trust-boundary decisions.

use crate::http::HttpConfig;
use std::collections::HashMap;
use std::net::SocketAddr;

/// One request's header block, keyed by the lowercased name `http_read` stores.
pub type HeaderMap = HashMap<String, String>;

/// The headers a proxy, a container runtime, or a tunnel adds when it delivers
/// someone else's request to a local socket. Presence is the whole signal.
const FORWARDING_HEADERS: [&str; 4] = ["forwarded", "x-forwarded-for", "x-real-ip", "via"];

/// Whether this configuration carries a credential source for its requests.
///
/// The decision is [`crate::http_auth::CredentialStore::is_credential_source`]:
/// a supplied store is a source even with no record, so the start guard refuses
/// that bind.
#[must_use]
pub fn has_credential_source(cfg: &HttpConfig) -> bool {
    cfg.credentials
        .as_ref()
        .is_some_and(|s| s.is_credential_source())
}

/// Why a listener is refused before it serves, or `None` when it is admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindRefusal {
    /// A loopback bind with no credential and no `insecure_no_auth` opt-out.
    Loopback,
    /// A bind outside loopback with no credential, unless both opt-outs are set.
    NonLoopback,
}

/// The refusal for this configuration and bind kind, or `None` when admitted.
///
/// A non-loopback bind exposes the listener to every interface, so it needs
/// both opt-outs: `insecure_no_auth` alone is the loopback development answer
/// and must not silently widen a bind.
///
/// A supplied store that holds no record authorizes nobody, so serving it would
/// answer 401 to every request. The bind is refused instead, whatever the
/// opt-outs say, because the opt-outs are for serving without credentials rather
/// than for serving a store that can never match one.
#[must_use]
pub fn bind_refusal(cfg: &HttpConfig, loopback: bool) -> Option<BindRefusal> {
    if cfg
        .credentials
        .as_ref()
        .is_some_and(|store| store.is_empty())
    {
        return Some(if loopback {
            BindRefusal::Loopback
        } else {
            BindRefusal::NonLoopback
        });
    }
    if has_credential_source(cfg) {
        return None;
    }
    if loopback {
        return (!cfg.insecure_no_auth).then_some(BindRefusal::Loopback);
    }
    let opted_out = cfg.insecure_no_auth && cfg.insecure_bind_any;
    (!opted_out).then_some(BindRefusal::NonLoopback)
}

/// Whether the listener this configuration and bind kind produce is reachable
/// by peers off this machine.
///
/// This establishes the listener's reachability and nothing about any one
/// caller: a loopback listener reached through a proxy is still this
/// predicate's "not reachable off this machine" answer, and
/// [`is_proxied_or_remote`] is what asks where a request actually came from.
///
/// An admitted bind is only reached after [`bind_refusal`] has passed, so this
/// answers about a listener that will serve: it is off-machine and it holds no
/// credential source, which is exactly the case where the operator's opt-outs
/// are serving every interface unauthenticated.
#[must_use]
pub fn serves_untrusted_network(cfg: &HttpConfig, loopback: bool) -> bool {
    !loopback && !has_credential_source(cfg)
}

/// Whether one request arrived from somewhere other than a local caller.
///
/// This establishes the caller's origin, to be read beside
/// [`serves_untrusted_network`]'s listener reachability and never in place of
/// it. A peer outside loopback is off this machine. A loopback peer is not
/// proof of a local caller: a reverse proxy terminating TLS, a published
/// container port, or a forwarded tunnel all deliver a remote request to a
/// loopback socket, and each marks the request with one of the forwarding
/// headers. Presence alone is the signal; the values are not parsed, because
/// they are unauthenticated text this server must not trust either way.
#[must_use]
pub fn is_proxied_or_remote(peer: SocketAddr, headers: &HeaderMap) -> bool {
    if !peer.ip().is_loopback() {
        return true;
    }
    headers.iter().any(|(name, _)| {
        FORWARDING_HEADERS
            .iter()
            .any(|known| name.trim().eq_ignore_ascii_case(known))
    })
}

#[cfg(test)]
#[path = "http_start_guard_tests.rs"]
mod http_start_guard_tests;
