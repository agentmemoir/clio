// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures of the candidate
//! `2026-07-28` HTTP conformance tests: a live candidate server on an
//! OS-assigned port, request helpers, and the result checkers both test
//! modules assert with.
//!
//! ## Owns
//! - The candidate fixture revision pin and the live-server helpers, including
//!   the one that serves the seeded domain-read state
//! - The candidate conditional-header builders and the POST helper
//! - The success, error, discovery, and removed-method checkers
//!
//! ## Does not own
//! - Any scenario or assertion (see `http_candidate_tests` and
//!   `http_candidate_guard_tests`)
//! - The candidate seam (`http_candidate`) or its rules (`stateless`,
//!   `conditional_http`, `method_guard`)
//!
//! ## Boundary
//!
//! Changes to how the candidate HTTP tests build or check a response should
//! normally require changes here. Changes to what a scenario asserts should
//! not.
//!
//! Keep this module free of test functions.

use crate::http::HttpConfig;
use crate::http::http_test_support_tests::{Response, post_json, request as raw_request};
use crate::http_auth::CredentialStore;
use crate::http_candidate::serve_candidate;
use crate::runtime::McpState;
use crate::stateless_fixtures::{domain_state, state};
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use serde_json::{Value, json};
use std::net::TcpListener;
use std::sync::Arc;

/// The candidate fixture pin: the revision a future activation test expects.
///
/// This is a test expectation, not a live protocol setting. The active pin
/// stays [`crate::MCP_PROTOCOL_REVISION`].
pub(crate) const CANDIDATE_FIXTURE_REVISION: &str = "2026-07-28";

/// Serve the candidate endpoint on an OS-assigned port and return that port.
///
/// The config opts out of the start-time credential requirement, so a scenario
/// that is not about auth keeps the credential-free loopback server. Per-request
/// auth still applies when the config carries a store.
pub(crate) fn spawn_candidate() -> u16 {
    spawn_candidate_with(HttpConfig {
        insecure_no_auth: true,
        ..HttpConfig::default()
    })
}

/// Serve the candidate endpoint with `cfg` on an OS-assigned port.
pub(crate) fn spawn_candidate_with(cfg: HttpConfig) -> u16 {
    spawn_candidate_state(state(), cfg)
}

/// Serve the candidate endpoint on an OS-assigned port over the seeded
/// domain-read state, so a read scenario reaches a store that holds the one
/// fixture item.
pub(crate) fn spawn_domain_candidate() -> u16 {
    spawn_candidate_state(
        domain_state(),
        HttpConfig {
            insecure_no_auth: true,
            ..HttpConfig::default()
        },
    )
}

/// Serve `state` with `cfg` on an OS-assigned port and return that port.
fn spawn_candidate_state(state: Arc<McpState>, cfg: HttpConfig) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        serve_candidate(state, cfg, listener);
    });
    port
}

/// A credential store with one record whose secret is `secret-token`.
pub(crate) fn single_record_store() -> CredentialStore {
    CredentialStore::from_records(vec![AuthTokenRecord {
        label: "ci".into(),
        secret_sha256: digest_hex("secret-token"),
        actor: Actor::Harness,
        banks: vec!["demo".into()],
        created_at: "2026-01-01T00:00:00Z".into(),
    }])
}

/// The candidate conditional headers for one method, ending the header block.
pub(crate) fn headers(method: &str) -> String {
    format!("Mcp-Protocol-Version: {CANDIDATE_FIXTURE_REVISION}\r\nMcp-Method: {method}\r\n")
}

/// The candidate headers for one tool call, carrying its resource name.
pub(crate) fn name_headers(method: &str, name: &str) -> String {
    format!(
        "Mcp-Protocol-Version: {CANDIDATE_FIXTURE_REVISION}\r\nMcp-Method: {method}\r\nMcp-Name: {name}\r\n"
    )
}

/// POST one candidate frame and return the response.
pub(crate) fn post(port: u16, frame: &Value, extra: &str) -> Response {
    let body = serde_json::to_string(frame).expect("a fixture serializes");
    post_json(port, &body, extra)
}

/// POST one candidate frame with an explicit path, content type, and `Accept`
/// value, so a gate scenario can vary each of them.
pub(crate) fn raw_post(
    port: u16,
    path: &str,
    content_type: &str,
    accept: &str,
    extra: &str,
    frame: &Value,
) -> Response {
    let body = serde_json::to_string(frame).expect("a fixture serializes");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\nAccept: {accept}\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    raw_request(port, &req)
}

/// Parse a response body as JSON.
pub(crate) fn body(response: &Response) -> Value {
    serde_json::from_str(&response.body).expect("the response body is JSON")
}

/// The envelope and discovery checkers the candidate tests assert with, shared
/// with the stdio scenarios so both transports check one definition.
pub(crate) use crate::stateless_fixtures::{
    check_complete_result, check_discovery_identity, check_error_envelope, check_success_envelope,
};

/// A removed method's answer: HTTP `404`, an error envelope with the not-found
/// code and a message naming the method, and no result at all.
///
/// The status is checked here, not only in a scenario, so a `200` success for a
/// method this server does not implement cannot pass the check.
pub(crate) fn check_removed_method_refusal(
    status: u16,
    envelope: &Value,
    method: &str,
) -> Result<(), String> {
    if status != 404 {
        return Err(format!(
            "`{method}`: status {status} is not 404 Not Found: {envelope}"
        ));
    }
    if envelope.get("result").is_some() {
        return Err(format!("`{method}` was served: {envelope}"));
    }
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("`{method}`: not JSON-RPC 2.0: {envelope}"));
    }
    if envelope["error"]["code"] != json!(-32601) {
        return Err(format!("`{method}`: not the not-found code: {envelope}"));
    }
    let message = envelope["error"]["message"]
        .as_str()
        .ok_or_else(|| format!("`{method}`: no error message: {envelope}"))?;
    if !message.contains(method) {
        return Err(format!(
            "`{method}`: the message does not name it: {envelope}"
        ));
    }
    Ok(())
}
