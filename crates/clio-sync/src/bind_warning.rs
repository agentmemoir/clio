// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one operator warning a sync-server bind
//! can raise: a bind to a non-loopback address is told what crosses the
//! network in the clear, and the bind proceeds.
//!
//! ## Owns
//! - `bind_is_loopback`: the single loopback classification for a bind address
//! - `public_bind_warning`: the full warning text an operator reads
//! - `warn_public_bind`: the stderr emission, once per bind, warn-and-continue
//!
//! ## Does not own
//! - The anonymous-bind refusal outside dev mode (`server`, mirrored by the
//!   MCP tool binding)
//! - Any request handling or transport framing (`server_conn`, `http`)
//! - The wire fields themselves (`protocol`, `client_feed`)
//!
//! ## Boundary
//!
//! Changes to what a public bind exposes should normally require changes here
//! together with the documentation that enumerates the same set. Changes to
//! whether a bind is allowed should not require changes here.
//!
//! Keep this module focused on the bind warning and nothing past it.
//!
//! # Why warn rather than refuse
//!
//! The dangerous case is already refused: a non-loopback bind without auth
//! material is rejected outside dev mode. An operator who has supplied a token
//! and bound publicly is running a supported configuration, typically behind a
//! proxy, so this warning names the exposure and leaves the decision with the
//! operator. It fires in every mode, dev included, because dev is where an
//! operator is most likely to try a public bind; it goes to stderr only, so a
//! machine-readable response stream is never touched.

use std::net::SocketAddr;

/// Whether a bind address keeps traffic on this machine.
///
/// Loopback is `127.0.0.0/8` and `::1` only, which is what the standard
/// library's `is_loopback` answers. The unspecified addresses `0.0.0.0` and
/// `::` are *not* loopback: they bind every interface, which is exactly the
/// case an operator gets wrong, so they warn.
pub(crate) fn bind_is_loopback(addr: SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// The warning text for a non-loopback bind.
///
/// It has three field clauses and a residual, in the order the wire presents
/// them. The five routing fields cross in the clear because the relay cannot
/// route, order, or authorize a row without them. The record identifier and the
/// content digest also cross in the clear, in a form derived under a key the
/// relay does not hold, so the relay can neither read them nor confirm a
/// guessed value against them; what it keeps is one stable value per row, and an
/// identifier the machine generated is sent unchanged rather than as such a
/// form. Only the content and the admission score are sealed. The residual —
/// the part the operator cannot mitigate — is named rather than presented as
/// harmless.
///
/// It never echoes a token, a key, or any secret, because it is printed before
/// any request arrives.
pub(crate) fn public_bind_warning(addr: SocketAddr) -> String {
    format!(
        "sync serve: bind {addr} is not loopback: bank scope, record kind, \
         operation, transaction time, and device id cross the network in the \
         clear, and the record-kind mix and device-id stability stay readable \
         (routing-necessary, not harmless); the record identifier and the \
         content digest cross in the clear too, in a form derived under a key \
         the relay does not hold, so a relay can neither read them nor confirm \
         a guessed value against them, though it does hold one stable value \
         per row, and an identifier the machine generated is sent unchanged \
         rather than as such a form; only the content and the admission score \
         are sealed; put a TLS-terminating reverse proxy in front or bind a \
         loopback address"
    )
}

/// Warn once per bind when the address is not loopback.
///
/// The warning is on stderr only, so an `--output json` stream or an MCP
/// response body is never corrupted. The bind always proceeds.
pub(crate) fn warn_public_bind(addr: SocketAddr) {
    if !bind_is_loopback(addr) {
        eprintln!("{}", public_bind_warning(addr));
    }
}

#[cfg(test)]
#[path = "bind_warning_tests.rs"]
mod bind_warning_tests;
