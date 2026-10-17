// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the start-time auth contract of the
//! Streamable HTTP transport: a bind with no credential source is refused
//! unless the operator opted out, and the opt-out restores the
//! credential-free server.
//!
//! ## Owns
//! - The loopback refusal message and its pinned substrings
//! - `insecure_no_auth` restoring the unauthenticated loopback server
//!
//! ## Does not own
//! - The non-loopback refusal (`http_tests`)
//! - The credential-edge rules themselves (`http_auth_tests`)
//! - The shared socket harness (`http_test_support_tests`)
//!
//! ## Boundary
//!
//! Changes to what a bind requires before it serves should normally require
//! changes here. Changes to how one request is authorized should not.
//!
//! Test-only. Never compiled into production builds.

use super::http_test_support_tests::{INIT, post_json, spawn_server};
use super::*;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;

/// A loopback bind with no credential source is refused at start unless the
/// operator opted out: the local dev default no longer serves anyone who can
/// reach the port.
#[test]
fn loopback_without_a_credential_is_refused_at_start() {
    let state = crate::runtime::McpState::open_with_effective(
        &crate::runtime::McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let err = serve_with_listener(state, HttpConfig::default(), listener);
    let message = err
        .expect_err("a loopback bind without a credential must fail closed")
        .to_string();

    assert!(
        message.contains("loopback bind requires an auth token"),
        "the pinned phrase must name the loopback case: {message}"
    );
    assert!(
        message.contains("--token-file"),
        "the refusal must name the token store: {message}"
    );
    assert!(
        message.contains("--insecure-no-auth"),
        "the refusal must name the opt-out flag: {message}"
    );
}

/// `insecure_no_auth` restores the credential-free loopback server, so an
/// operator who asked for it still serves a request with no header.
#[test]
fn insecure_no_auth_restores_the_unauthenticated_loopback_server() {
    let (port, _) = spawn_server(HttpConfig {
        insecure_no_auth: true,
        ..HttpConfig::default()
    });
    assert_eq!(post_json(port, INIT, "").status, 200);
}

/// With both `--insecure-no-auth` and a credential store, the store still
/// governs every request: the help promises the store is enforced, and the
/// request path must match. Only the start guard sees the opt-out.
#[test]
fn insecure_no_auth_with_a_credential_store_still_requires_the_credential() {
    let secret = clio_config::auth_tokens::generate_secret();
    let store = CredentialStore::from_records(vec![AuthTokenRecord {
        label: "ci".into(),
        secret_sha256: digest_hex(&secret),
        actor: Actor::Harness,
        banks: vec!["demo".into()],
        created_at: "2026-10-01T00:00:00Z".into(),
    }]);
    let (port, _) = spawn_server(HttpConfig {
        insecure_no_auth: true,
        credentials: Some(Arc::new(store)),
        ..HttpConfig::default()
    });
    assert_eq!(
        post_json(port, INIT, "").status,
        401,
        "a request with no credential is refused even under the opt-out"
    );
    let auth = format!("Authorization: Bearer {secret}\r\n");
    assert_eq!(
        post_json(port, INIT, &auth).status,
        200,
        "the store's own credential is accepted"
    );
}
