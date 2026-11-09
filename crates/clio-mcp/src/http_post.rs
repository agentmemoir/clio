// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one dispatch decision a Streamable HTTP
//! route makes after its gates pass: how an admitted POST body is answered.
//!
//! ## Owns
//! - The per-request context a strategy reads
//! - The dispatch strategy trait
//! - The request-scoped state a bound request runs against
//!
//! ## Does not own
//! - The request gates: verbs, Origin, auth, path, and media types (see `http`)
//! - The stateless admission and dispatch the strategy calls (see `stateless`)
//! - The strategy the transport installs (see `http_candidate`)
//!
//! ## Boundary
//!
//! Changes to how one admitted POST is answered should normally require
//! changes here. Changes to a gate or a tool should not.
//!
//! Keep this module focused on the POST dispatch decision.

use crate::http_auth::AuthOutcome;
use crate::http_read::HttpRequest;
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use std::sync::Arc;

/// The status, body, and extra headers one route answers with.
pub(crate) type PostResponse = (u16, String, Vec<(String, String)>);

/// The inputs one POST dispatch strategy reads.
pub(crate) struct PostContext<'a> {
    /// The request-scoped state, already carrying the request's principal.
    pub(crate) state: &'a McpState,
    /// The parsed request, including its body and headers.
    pub(crate) req: &'a HttpRequest,
}

/// The one dispatch decision a route delegates after its gates pass.
pub(crate) trait PostRoute {
    /// Answer the admitted POST body described by `ctx`.
    fn respond(&self, ctx: PostContext<'_>) -> PostResponse;
}

/// The state one request dispatches against: a shallow clone of the base state
/// carrying this request's principal.
///
/// The clone is the point. Every field that must stay single and unforked is
/// already behind an `Arc` on `McpState`, so a per-request copy shares the store
/// engines, the index drain, the retention store, and the config runtime instead
/// of duplicating them.
pub(crate) fn scoped_state(handler: &Arc<McpHandler>, principal: AuthOutcome) -> McpState {
    let base = handler.state();
    match principal {
        AuthOutcome::Bound(principal) => base.with_principal(principal),
        AuthOutcome::Unbound => base.as_ref().clone(),
    }
}

#[cfg(test)]
#[path = "http_post_tests.rs"]
mod http_post_tests;
