// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one dispatch decision a Streamable HTTP
//! route makes after its gates pass: how an admitted POST body is answered.
//! The active transport answers through the pinned protocol entry point and
//! mints a session identifier for `initialize`; another transport can supply
//! its own strategy and still run every gate in `http`.
//!
//! ## Owns
//! - The per-request context a strategy reads
//! - The dispatch strategy trait
//! - The active transport's strategy: the bound protocol entry point and the
//!   session mint its `initialize` answer carries
//! - The request-scoped state the bound entry point runs against
//!
//! ## Does not own
//! - The request gates: verbs, Origin, auth, path, media types, and sessions
//!   (see `http`)
//! - The pinned protocol entry point (see `protocol`)
//! - The candidate strategy (see `http_candidate`, test-only)
//!
//! ## Boundary
//!
//! Changes to how one admitted POST is answered should normally require
//! changes here. Changes to a gate or a tool should not.
//!
//! Keep this module focused on the POST dispatch decision.

use crate::http::HttpConfig;
use crate::http_auth::AuthOutcome;
use crate::http_guard::{HttpShared, mint_session};
use crate::http_read::HttpRequest;
use crate::protocol::McpHandler;
use crate::runtime::McpState;
use std::sync::Arc;

/// The status, body, and extra headers one route answers with.
pub(crate) type PostResponse = (u16, String, Vec<(String, String)>);

/// The inputs one POST dispatch strategy reads.
pub(crate) struct PostContext<'a> {
    /// The handler that owns the protocol entry point.
    pub(crate) handler: &'a Arc<McpHandler>,
    /// The request-scoped state, already carrying the request's principal.
    pub(crate) state: &'a McpState,
    /// The transport configuration the route gates used.
    pub(crate) cfg: &'a HttpConfig,
    /// The per-server session registry.
    pub(crate) shared: &'a HttpShared,
    /// The parsed request, including its body and headers.
    pub(crate) req: &'a HttpRequest,
    /// The identity of the requesting principal, if the request is bound.
    pub(crate) identity: Option<&'a str>,
    /// The parsed JSON-RPC method, or the empty string when none was read.
    pub(crate) request_method: &'a str,
}

/// The one dispatch decision a route delegates after its gates pass.
pub(crate) trait PostRoute {
    /// Answer the admitted POST body described by `ctx`.
    fn respond(&self, ctx: PostContext<'_>) -> PostResponse;
}

/// The active transport's strategy: the pinned protocol entry point, with a
/// session identifier minted for an `initialize` answer.
pub(crate) struct LivePost;

impl PostRoute for LivePost {
    fn respond(&self, ctx: PostContext<'_>) -> PostResponse {
        let Some(response) = ctx
            .handler
            .handle_frame_with_state(ctx.state, ctx.req.body.trim())
        else {
            // Notification-only POST: 202 with no body (pinned revision rule).
            return (202, String::new(), Vec::new());
        };
        let mut extra = Vec::new();
        if ctx.cfg.sessions_enabled && ctx.request_method == "initialize" {
            let id = mint_session(ctx.shared, ctx.identity);
            extra.push(("MCP-Session-Id".into(), id));
        }
        (
            200,
            serde_json::to_string(&response).unwrap_or_default(),
            extra,
        )
    }
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
