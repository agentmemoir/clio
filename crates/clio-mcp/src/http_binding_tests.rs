// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the session-binding tests: a minted session
//! belongs to the principal that ran `initialize`, is reused only by that
//! principal, and is deleted only by that principal.
//!
//! ## Owns
//! - Mint / reuse / cross-principal / DELETE behavior over the real bound
//!   server, with credentials injected as literals
//!
//! ## Does not own
//! - The registry-level predicate and the guards themselves (`http_guard_tests`)
//! - The session lifecycle on an unbound server (see `http_tests`, whose
//!   lifecycle test already pins that unchanged behavior)
//! - The transport harness (see `http_test_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::http_test_support_tests::{INIT, post_json, request, spawn_server};
use super::*;
use crate::http_auth::CredentialStore;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use std::sync::Arc;

/// A store with one record per (label, secret), built without a file. The
/// secrets are literals, so no test reads the host environment.
fn store_with(records: &[(&str, &str)]) -> CredentialStore {
    CredentialStore::from_records(
        records
            .iter()
            .map(|(label, secret)| AuthTokenRecord {
                label: (*label).to_owned(),
                secret_sha256: digest_hex(secret),
                actor: Actor::Harness,
                banks: vec!["demo".to_owned()],
                created_at: "2026-01-01T00:00:00Z".to_owned(),
            })
            .collect(),
    )
}

/// The bound server this suite speaks to: two principals, sessions on.
fn bound_sessions_server(store: CredentialStore) -> (u16, Arc<McpHandler>) {
    spawn_server(HttpConfig {
        credentials: Some(Arc::new(store)),
        sessions_enabled: true,
        ..HttpConfig::default()
    })
}

fn bearer(secret: &str) -> String {
    format!("Authorization: Bearer {secret}\r\n")
}

const LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

#[test]
fn a_bound_session_is_reused_by_its_owner_and_refused_for_another() {
    let store = store_with(&[("ci", "secret-ci"), ("bot", "secret-bot")]);
    let (port, _) = bound_sessions_server(store);

    let mint = post_json(port, INIT, &bearer("secret-ci"));
    assert_eq!(mint.status, 200);
    let session = mint
        .headers
        .get("mcp-session-id")
        .cloned()
        .expect("a bound server mints on initialize");
    let with_session = format!("MCP-Session-Id: {session}\r\n");

    let owner = post_json(
        port,
        LIST,
        &format!("{}{with_session}", bearer("secret-ci")),
    );
    assert_eq!(owner.status, 200, "the owner may reuse the session");
    let foreign = post_json(
        port,
        LIST,
        &format!("{}{with_session}", bearer("secret-bot")),
    );
    assert_eq!(foreign.status, 404, "another principal must be refused");
    // The refusal removed nothing: the owner's session is still live.
    let again = post_json(
        port,
        LIST,
        &format!("{}{with_session}", bearer("secret-ci")),
    );
    assert_eq!(again.status, 200, "the session survives a foreign attempt");
}

#[test]
fn delete_is_owner_only_over_the_wire() {
    let store = store_with(&[("ci", "secret-ci"), ("bot", "secret-bot")]);
    let (port, _) = bound_sessions_server(store);

    let mint = post_json(port, INIT, &bearer("secret-ci"));
    let session = mint
        .headers
        .get("mcp-session-id")
        .cloned()
        .expect("a bound server mints on initialize");
    let del = |auth: &str| {
        request(
            port,
            &format!(
                "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\n{auth}MCP-Session-Id: {session}\r\nContent-Length: 0\r\n\r\n"
            ),
        )
    };

    let foreign = del(&bearer("secret-bot"));
    assert_eq!(foreign.status, 404, "a foreign delete must be refused");
    let owner = del(&bearer("secret-ci"));
    assert_eq!(owner.status, 200, "the owner's delete must remove it");
    let after = post_json(
        port,
        LIST,
        &format!("{}MCP-Session-Id: {session}\r\n", bearer("secret-ci")),
    );
    assert_eq!(after.status, 404, "the session is gone after the delete");
}
