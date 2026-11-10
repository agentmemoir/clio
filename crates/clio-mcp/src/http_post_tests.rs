// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the POST dispatch strategy
//! and the request-scoped state a bound request runs against.
//!
//! ## Owns
//! - The strategy trait's notification, refused-method, and served branches
//! - `scoped_state` and the principal it carries
//! - The `Debug` rendering of both body kinds
//!
//! ## Does not own
//! - The strategy trait or the route gates (see `http_post` and `http`)
//! - The stateless admission the strategy calls (see `stateless`)
//!
//! ## Boundary
//!
//! Changes to how the transport answers one admitted POST should normally
//! require changes here. Changes to a gate or a tool should not.

use super::*;
use crate::event_stream::{EventStream, FrameSource, frame};
use crate::http_candidate::CandidatePost;
use crate::principal::Principal;
use crate::runtime::McpOpenOptions;
use clio_types::Actor;
use serde_json::json;
use std::time::Duration;

/// A fresh runtime state for one test.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state")
}

/// One parsed request carrying `body` and `headers`.
fn request(body: &str, headers: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/mcp".into(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: body.into(),
    }
}

/// The bytes of a buffered answer, which is the only kind the candidate sends.
fn buffered(body: PostBody) -> String {
    match body {
        PostBody::Buffered(bytes) => bytes,
        PostBody::Stream(_) => panic!("the candidate answers with a buffered body"),
    }
}

/// The headers a well-formed stateless request carries.
fn stateless_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("mcp-method", "tools/list"),
        ("mcp-protocol-version", crate::MCP_PROTOCOL_REVISION),
    ]
}

#[test]
fn a_notification_is_answered_with_202_and_no_body() {
    let state = state();
    let req = request(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        &[
            ("mcp-method", "notifications/initialized"),
            ("mcp-protocol-version", crate::MCP_PROTOCOL_REVISION),
        ],
    );
    let (status, body, extra) = CandidatePost.respond(PostContext {
        state: &state,
        req: &req,
    });
    let body = buffered(body);
    assert_eq!(status, 202);
    assert!(body.is_empty(), "{body}");
    assert!(extra.is_empty(), "no header is answered with a body alone");
}

#[test]
fn a_removed_method_is_answered_with_404() {
    let state = state();
    let req = request(
        r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        &[
            ("mcp-method", "ping"),
            ("mcp-protocol-version", crate::MCP_PROTOCOL_REVISION),
        ],
    );
    let (status, body, extra) = CandidatePost.respond(PostContext {
        state: &state,
        req: &req,
    });
    let body = buffered(body);
    assert_eq!(status, 404, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("a JSON answer");
    assert_eq!(value["error"]["code"], json!(-32601), "{value}");
    assert!(extra.is_empty(), "{extra:?}");
}

#[test]
fn a_stateless_listing_is_served_with_200() {
    let state = state();
    let req = request(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        &stateless_headers(),
    );
    let (status, body, extra) = CandidatePost.respond(PostContext {
        state: &state,
        req: &req,
    });
    let body = buffered(body);
    assert_eq!(status, 200, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("a JSON answer");
    assert!(value["result"]["tools"].is_array(), "{value}");
    assert!(extra.is_empty(), "{extra:?}");
}

#[test]
fn scoped_state_carries_the_request_principal() {
    let state = state();
    let handler = Arc::new(McpHandler::new(Arc::clone(&state)));
    let unbound = scoped_state(&handler, AuthOutcome::Unbound);
    assert!(
        unbound.authorize_admin().is_ok(),
        "an unbound state is local"
    );
    let bound = scoped_state(
        &handler,
        AuthOutcome::Bound(Principal::new(
            "agent-label".into(),
            Actor::Agent,
            Vec::new(),
        )),
    );
    assert!(
        bound.authorize_admin().is_err(),
        "the bound principal's actor decides the administrative rule"
    );
}

#[test]
fn a_buffered_body_renders_its_payload() {
    let body = PostBody::Buffered("ready".into());
    assert_eq!(format!("{body:?}"), "Buffered(\"ready\")");
}

#[test]
fn a_streamed_body_hides_its_source() {
    struct Silent;
    impl FrameSource for Silent {
        fn next(&mut self, _timeout: Duration) -> Option<Vec<u8>> {
            None
        }
    }
    let body = PostBody::Stream(EventStream {
        head: frame("{}"),
        source: Box::new(Silent),
    });
    assert_eq!(
        format!("{body:?}"),
        "Stream(..)",
        "a stream's source is not rendered"
    );
}
