// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bind admission decision of the sync
//! server: whether a resolved bind address may serve at all, given the
//! operator's explicit permission to bind outside loopback.
//!
//! ## Owns
//! - `bind_admission`: the single decision that refuses a non-loopback bind
//!   unless the operator setting allows it
//! - `BindRefusal`: why a bind is refused
//! - `refusal_message`: the operator-facing text for that refusal
//!
//! ## Does not own
//! - The loopback classification itself (`bind_warning::bind_is_loopback`)
//! - The public-bind warning, which speaks only about an admitted bind
//!   (`bind_warning::warn_public_bind`)
//! - Binding the socket, and the anonymous-bind refusal that precedes this
//!   decision (`server`)
//!
//! ## Boundary
//!
//! Changes to what a bind must satisfy before it serves should normally require
//! changes here. Changes to what an admitted public bind exposes should not:
//! that belongs to `bind_warning`.
//!
//! Keep this module focused on the start-time decision and nothing past it.

use std::net::SocketAddr;

/// Why a bind is refused before it serves, or `None` when it is admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindRefusal {
    /// A bind outside loopback with the operator setting off.
    NonLoopbackNotAllowed,
}

/// The refusal for this bind kind and setting, or `None` when admitted.
///
/// A bind outside loopback is reachable from every interface, so it needs the
/// operator's explicit permission; a loopback bind needs none.
#[must_use]
pub fn bind_admission(loopback: bool, allow_public_bind: bool) -> Option<BindRefusal> {
    (!loopback && !allow_public_bind).then_some(BindRefusal::NonLoopbackNotAllowed)
}

/// The refusal text for a refused bind: the address, the rule, and the setting.
#[must_use]
pub fn refusal_message(addr: SocketAddr) -> String {
    format!(
        "sync serve: refusing the bind at {addr}: it is not loopback, and a \
         non-loopback bind is refused unless the operator allows it (set \
         sync.allow_public_bind to true to serve an address reachable from \
         other hosts)"
    )
}

#[cfg(test)]
#[path = "bind_admission_tests.rs"]
mod bind_admission_tests;
