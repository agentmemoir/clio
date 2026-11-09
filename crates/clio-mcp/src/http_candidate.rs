// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Streamable HTTP POST strategy: it answers
//! an admitted POST body through the stateless admission, so an HTTP request
//! runs every gate the accept loop runs (verbs, Origin, auth, path, media types)
//! and then the revision's request rules.
//!
//! ## Owns
//! - The POST dispatch strategy: the stateless admission, and the status each of
//!   its outcomes answers with
//!
//! ## Does not own
//! - The accept loop, request reading, and every request gate (see `http`)
//! - The verb decision, the conditional headers, and the admission rules (see
//!   `method_guard`, `conditional_http`, and `stateless`)
//! - The stateless request rules themselves (see `stateless`)
//!
//! ## Boundary
//!
//! Changes to how the transport answers an admitted POST should normally
//! require changes here. Changes to a gate or a tool should not.
//!
//! Keep this module focused on the POST strategy.

use crate::http_post::{PostContext, PostResponse, PostRoute};
use crate::protocol::ERR_METHOD_NOT_FOUND;
use serde_json::Value;

/// HTTP status for a request whose method this server does not implement.
///
/// The stateless transport answers `404 Not Found` with a JSON-RPC `-32601`
/// body, which tells a modern client apart from a legacy endpoint that is not
/// hosted at all.
const METHOD_NOT_FOUND_STATUS: u16 = 404;

/// The POST strategy: the stateless admission answers every body.
pub(crate) struct CandidatePost;

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
