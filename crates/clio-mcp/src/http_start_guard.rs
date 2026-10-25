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
#[must_use]
pub fn bind_refusal(cfg: &HttpConfig, loopback: bool) -> Option<BindRefusal> {
    if has_credential_source(cfg) {
        return None;
    }
    if loopback {
        return (!cfg.insecure_no_auth).then_some(BindRefusal::Loopback);
    }
    let opted_out = cfg.insecure_no_auth && cfg.insecure_bind_any;
    (!opted_out).then_some(BindRefusal::NonLoopback)
}
