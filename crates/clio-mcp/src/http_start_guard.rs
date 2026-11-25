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
//! - `serves_untrusted_network`: the fact a served bind passes to its handler,
//!   read from the same credential-source notion `bind_refusal` uses
//!
//! ## Does not own
//! - Request authentication, which still governs every request whatever the
//!   start-time opt-outs (see `http_auth`)
//! - Binding the socket and reporting the refusal to the operator (see `http`)
//! - Origin validation and session lifecycle (see `http_guard`)
//!
//! ## Boundary
//!
//! Changes to what a bind requires before it serves should normally require
//! changes here. Changes to how one request is authorized should not.
//!
//! Keep this module focused on the start-time decision and nothing past it.

use crate::http::{HttpConfig, has_credential_source};

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

/// Whether the listener this configuration and bind kind produce answers peers
/// off this machine without a credential to vouch for who they are.
///
/// An admitted bind is only reached after [`bind_refusal`] has passed, so this
/// answers about a listener that will serve: it is off-machine and it holds no
/// credential source, which is exactly the case where the operator's opt-outs
/// are serving every interface unauthenticated.
#[must_use]
pub fn serves_untrusted_network(cfg: &HttpConfig, loopback: bool) -> bool {
    !loopback && !has_credential_source(cfg)
}

#[cfg(test)]
#[path = "http_start_guard_tests.rs"]
mod http_start_guard_tests;
