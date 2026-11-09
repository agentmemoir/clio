// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for a non-activated candidate Streamable HTTP
//! entry point. It supplies the candidate POST strategy to the live accept
//! loop, so the candidate runs every gate the active transport runs (verbs,
//! Origin, auth, path, media types, and sessions) and differs only in how an
//! admitted POST body is answered.
//!
//! ## Owns
//! - The candidate POST dispatch strategy: the candidate admission, and the
//!   status each of its outcomes answers with
//!
//! ## Does not own
//! - The accept loop, request reading, and every request gate (see `http`)
//! - The verb decision, the conditional headers, and the admission rules (see
//!   `method_guard`, `conditional_http`, and `stateless`)
//! - The candidate revision's request rules (see `stateless`)
//! - Activation: only this crate's tests compile and reach this module
//!
//! ## Boundary
//!
//! Changes to how the candidate answers an admitted POST should normally
//! require changes here. Changes to the live transport or a tool should not.
//!
//! Keep this module focused on the candidate's POST strategy.

use crate::http::{HttpConfig, serve_with_listener_and};
use crate::http_post::{PostContext, PostResponse, PostRoute};
use crate::protocol::ERR_METHOD_NOT_FOUND;
use crate::runtime::McpState;
use serde_json::Value;
use std::net::TcpListener;
use std::sync::Arc;

/// HTTP status for a request whose method this server does not implement.
///
/// The 2026-07-28 transport answers `404 Not Found` with a JSON-RPC `-32601`
/// body, which tells a modern client apart from a legacy endpoint that is not
/// hosted at all.
const METHOD_NOT_FOUND_STATUS: u16 = 404;

/// Serve the candidate endpoint over an already-bound listener until it fails.
///
/// The listener, the gates, and the writer are the live transport's; only the
/// POST strategy is the candidate's.
pub(crate) fn serve_candidate(state: Arc<McpState>, cfg: HttpConfig, listener: TcpListener) {
    let _ = serve_with_listener_and(state, cfg, listener, Arc::new(CandidatePost));
}

/// The candidate POST strategy: the candidate admission answers every body.
struct CandidatePost;

impl PostRoute for CandidatePost {
    fn respond(&self, ctx: PostContext<'_>) -> PostResponse {
        match crate::stateless::admit(ctx.state, |header| ctx.req.get(header), ctx.req.body.trim())
        {
            Ok(Some(response)) => {
                let status = method_not_found_status(&response);
                (
                    status,
                    serde_json::to_string(&response).unwrap_or_default(),
                    Vec::new(),
                )
            }
            Ok(None) => (202, String::new(), Vec::new()),
            Err((status, body)) => (status, body, Vec::new()),
        }
    }
}

/// The HTTP status for one admitted answer: `404` when this server does not
/// implement the requested method, `200` otherwise.
fn method_not_found_status(response: &Value) -> u16 {
    if response["error"]["code"].as_i64() == Some(ERR_METHOD_NOT_FOUND) {
        METHOD_NOT_FOUND_STATUS
    } else {
        200
    }
}

#[cfg(test)]
#[path = "http_candidate_tests.rs"]
mod http_candidate_tests;

#[cfg(test)]
#[path = "http_candidate_guard_tests.rs"]
mod http_candidate_guard_tests;

#[cfg(test)]
#[path = "http_candidate_listing_tests.rs"]
mod http_candidate_listing_tests;
