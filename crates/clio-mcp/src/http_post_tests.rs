// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the POST dispatch
//! strategies: the active transport's answer through the pinned protocol entry
//! point, the session identifier its `initialize` answer carries, and the
//! request-scoped state a bound request runs against.
//!
//! ## Owns
//! - Every assertion about `LivePost`, its notification and session branches,
//!   and `scoped_state`
//!
//! ## Does not own
//! - The strategy trait or the route gates (see `http_post` and `http`)
//! - The candidate strategy (see `http_candidate`)
//!
//! ## Boundary
//!
//! Changes to how the active transport answers one admitted POST should
//! normally require changes here. Changes to a gate or a tool should not.

use super::*;
use crate::principal::Principal;
use crate::runtime::McpOpenOptions;
use clio_types::Actor;
use serde_json::json;
use std::collections::HashMap;

/// A fresh runtime state for one test.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state")
}

/// One parsed request carrying `body`.
fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/mcp".into(),
        headers: HashMap::new(),
        body: body.into(),
    }
}

#[test]
fn live_post_answers_the_pinned_protocol_entry_point() {
    let state = state();
    let handler = Arc::new(McpHandler::new(Arc::clone(&state)));
    let cfg = HttpConfig::default();
    let shared = HttpShared::default();
    let req = request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    let (status, body, extra) = LivePost.respond(PostContext {
        handler: &handler,
        state: &state,
        cfg: &cfg,
        shared: &shared,
        req: &req,
        identity: None,
        request_method: "tools/list",
    });
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&body).expect("a JSON answer");
    assert!(value["result"].is_object(), "{value}");
    assert!(extra.is_empty(), "no session is minted for `tools/list`");
}

#[test]
fn live_post_answers_a_notification_with_202_and_no_body() {
    let state = state();
    let handler = Arc::new(McpHandler::new(Arc::clone(&state)));
    let cfg = HttpConfig::default();
    let shared = HttpShared::default();
    let req = request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    let (status, body, extra) = LivePost.respond(PostContext {
        handler: &handler,
        state: &state,
        cfg: &cfg,
        shared: &shared,
        req: &req,
        identity: None,
        request_method: "notifications/initialized",
    });
    assert_eq!(status, 202);
    assert!(body.is_empty(), "{body}");
    assert_eq!(extra, Vec::new(), "a notification mints no session");
}

#[test]
fn live_post_mints_a_session_for_initialize() {
    let state = state();
    let handler = Arc::new(McpHandler::new(Arc::clone(&state)));
    let cfg = HttpConfig {
        sessions_enabled: true,
        ..HttpConfig::default()
    };
    let shared = HttpShared::default();
    let req = request(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    let (status, _body, extra) = LivePost.respond(PostContext {
        handler: &handler,
        state: &state,
        cfg: &cfg,
        shared: &shared,
        req: &req,
        identity: Some("fixture"),
        request_method: "initialize",
    });
    assert_eq!(status, 200);
    assert_eq!(extra.len(), 1, "{extra:?}");
    assert_eq!(extra[0].0, "MCP-Session-Id");
    assert!(shared.is_live(&extra[0].1, Some("fixture")));
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
