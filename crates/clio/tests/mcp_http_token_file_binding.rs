// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live credential contract of `clio mcp
//! http`: which credential class a real served process answers under, that a
//! credential dropped from the store is refused at the next start, that a
//! loopback start with no token file mints and requires a one-time token, and
//! that `--insecure-no-auth` restores the credential-free local server.
//!
//! ## Owns
//! - The two- and one-record store writers
//! - The per-credential-class probe helper
//! - The live server answers, the revocation-at-restart check, the legacy-flag
//!   check, and the unchanged default
//!
//! ## Does not own
//! - The harness (`mcp_http_token_file_support`)
//! - The token the production environment read serves
//!   (`mcp_http_env_token_binding`)
//! - The start-up refusals (`mcp_http_token_file_startup`)
//! - The in-process credential binding (`clio-mcp`'s `mcp_principal_binding`)
//!
//! ## Boundary
//!
//! Changes to what a served request answers, or to how a credential is revoked,
//! should normally require changes here. Changes to what a start refuses should
//! not.
//!
//! Keep this module focused on the served path, after the process has bound.

mod common;
#[path = "mcp_http_token_file_support/mod.rs"]
mod mcp_http_token_file_support;

use mcp_http_token_file_support::{
    Server, Store, bearer, call, digests_on_disk, forbidden, minted_token, ok, post,
};
use serde_json::json;
// Only the store writers tighten a file's mode, so the import is gated with them.
#[cfg(unix)]
use std::fs;

/// Write a two-record store: one harness, one agent, both at `demo`.
fn write_two_record_store(store: &Store, harness_secret: &str, agent_secret: &str) {
    use clio_config::auth_tokens::{AuthTokenRecord, digest_hex, save_auth_tokens};
    use clio_types::Actor;
    save_auth_tokens(
        &store.path,
        &[
            AuthTokenRecord {
                label: "ci".into(),
                secret_sha256: digest_hex(harness_secret),
                actor: Actor::Harness,
                banks: vec!["demo".into()],
                created_at: "2026-10-01T00:00:00Z".into(),
            },
            AuthTokenRecord {
                label: "bot".into(),
                secret_sha256: digest_hex(agent_secret),
                actor: Actor::Agent,
                banks: vec!["demo".into()],
                created_at: "2026-10-01T00:00:00Z".into(),
            },
        ],
    )
    .expect("save");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o600)).expect("tighten");
    }
}

/// Write a one-record store: one harness credential, at `demo`.
///
/// The same writer and the same owner-only mode as the two-record helper, so
/// rewriting the same path between two server starts is the change an operator
/// makes when they revoke one credential.
fn write_one_record_store(store: &Store, label: &str, secret: &str) {
    use clio_config::auth_tokens::{AuthTokenRecord, digest_hex, save_auth_tokens};
    use clio_types::Actor;
    save_auth_tokens(
        &store.path,
        &[AuthTokenRecord {
            label: label.into(),
            secret_sha256: digest_hex(secret),
            actor: Actor::Harness,
            banks: vec!["demo".into()],
            created_at: "2026-10-01T00:00:00Z".into(),
        }],
    )
    .expect("save");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o600)).expect("tighten");
    }
}

/// One credential class's answer to two probes, plus the leak check.
fn assert_credential_class(
    port: u16,
    name: &str,
    header: &str,
    want_status: u16,
    want_forbidden: bool,
    secrets: &[String],
    digests: &[String],
) {
    let export = call(
        "export",
        json!({"bank": "demo", "destination": "/dev/null"}),
    );
    let (status, body) = post(port, &export, header);
    assert_eq!(status, want_status, "{name}: {body}");
    if want_status == 401 {
        assert!(
            !body.contains("structuredContent"),
            "{name} reached a tool: {body}"
        );
    } else {
        assert_eq!(forbidden(&body), want_forbidden, "{name}: {body}");
    }

    // The bank escape is refused for every credential that got this far.
    let escape = call(
        "store",
        json!({
            "bank": "not-listed",
            "item": {"id": "escape", "gist": "must not land"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    );
    let (status, body) = post(port, &escape, header);
    assert_eq!(status, want_status, "{name} bank escape: {body}");
    if want_status == 200 {
        assert!(forbidden(&body), "{name} bank escape: {body}");
    } else {
        assert!(
            !body.contains("structuredContent"),
            "{name} reached a tool: {body}"
        );
    }

    // Nothing a caller can see carries secret or digest material.
    for secret in secrets {
        assert!(!body.contains(secret.as_str()), "{name} leaked a secret");
    }
    for digest in digests {
        assert!(!body.contains(digest.as_str()), "{name} leaked a digest");
    }
}

#[test]
fn a_live_server_answers_under_each_credential_class() {
    // The live transcript: harness, agent, unknown, and no credential, against
    // one real `clio mcp http` process.
    let store = Store::new("live");
    let harness_secret = clio_config::auth_tokens::generate_secret();
    let agent_secret = clio_config::auth_tokens::generate_secret();
    write_two_record_store(&store, &harness_secret, &agent_secret);
    let digests = digests_on_disk(&store.path);
    let server = Server::start(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert_ne!(server.port, 0, "the server must announce a port");
    let port = server.port;

    let unknown = clio_config::auth_tokens::generate_secret();
    let every_secret = vec![
        harness_secret.clone(),
        agent_secret.clone(),
        unknown.clone(),
    ];
    // (credential class, header, expected status, whether export is refused)
    let cases: Vec<(&str, String, u16, bool)> = vec![
        ("harness record", bearer(&harness_secret), 200, false),
        ("agent record", bearer(&agent_secret), 200, true),
        ("unknown secret", bearer(&unknown), 401, false),
        ("no credential", String::new(), 401, false),
    ];
    for (name, header, want_status, want_forbidden) in cases {
        assert_credential_class(
            port,
            name,
            &header,
            want_status,
            want_forbidden,
            &every_secret,
            &digests,
        );
    }

    // A harness record may still write inside its own bank.
    let write = call(
        "store",
        json!({
            "bank": "demo",
            "item": {"id": "live-1", "gist": "may land"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    );
    let (status, body) = post(port, &write, &bearer(&harness_secret));
    assert_eq!(status, 200, "{body}");
    assert!(ok(&body), "{body}");

    // The server's own stderr must be free of secret and digest material too.
    let diagnostics = server.stderr_text();
    for secret in &every_secret {
        assert!(
            !diagnostics.contains(secret.as_str()),
            "server stderr leaked a secret: {diagnostics}"
        );
    }
    for digest in &digests {
        assert!(
            !diagnostics.contains(digest.as_str()),
            "server stderr leaked a digest: {diagnostics}"
        );
    }

    let stdout = server.stop();
    assert!(
        stdout.is_empty(),
        "stdout must carry only MCP data: {:?}",
        String::from_utf8_lossy(&stdout)
    );
}

/// A record removed from the store is refused by the next server, not the one
/// that loaded it.
#[test]
fn a_credential_removed_from_the_store_is_401_at_the_next_start() {
    let store = Store::new("revoked");
    let removed = clio_config::auth_tokens::generate_secret();
    write_one_record_store(&store, "ci", &removed);

    let probe = call(
        "store",
        json!({
            "bank": "demo",
            "item": {"id": "revoked-1", "gist": "served while the record is present"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    );

    let first = Server::start(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert_ne!(first.port, 0, "the server must announce a port");
    let (status, body) = post(first.port, &probe, &bearer(&removed));
    assert_eq!(status, 200, "{body}");
    assert!(ok(&body), "{body}");
    // The first child is killed and reaped here, so the second start is a real
    // restart of the binary rather than a second server beside the first.
    let _ = first.stop();

    // A different record survives, so the store still loads and authorizes
    // somebody: the 401 below can only be the revoked credential, not a store
    // that stopped working.
    let surviving = clio_config::auth_tokens::generate_secret();
    write_one_record_store(&store, "bot", &surviving);

    let second = Server::start(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert_ne!(second.port, 0, "the server must announce a port");

    let (status, body) = post(second.port, &probe, &bearer(&removed));
    assert_eq!(status, 401, "a removed credential must be refused: {body}");
    assert!(
        !body.contains("structuredContent"),
        "the removed credential reached a tool: {body}"
    );

    let (status, body) = post(second.port, &probe, &bearer(&surviving));
    assert_eq!(
        status, 200,
        "the surviving credential must still be served: {body}"
    );
    let _ = second.stop();
}

/// A loopback server with no `--token-file` mints a one-time token, requires it,
/// and serves only the client that presents it.
#[test]
fn a_loopback_server_mints_a_token_and_requires_it() {
    let server = Server::start(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
    ]);
    assert_ne!(server.port, 0, "the server must announce a port");
    let token = minted_token(&server.stderr_text())
        .expect("a loopback server with no --token-file must mint a token");
    let port = server.port;
    let body = call(
        "store",
        json!({
            "bank": "default",
            "item": {"id": "minted-1", "gist": "the owner may write here"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    );

    // No credential: the minted server refuses, unlike the old dev default.
    let (status, response) = post(port, &body, "");
    assert_eq!(status, 401, "{response}");
    assert!(
        !response.contains("structuredContent"),
        "an unauthenticated request reached a tool: {response}"
    );

    // Only the minted token serves, and nothing echoes it back.
    let (status, response) = post(port, &body, &bearer(&token));
    assert_eq!(status, 200, "{response}");
    assert!(ok(&response), "{response}");
    assert!(
        !response.contains(&token),
        "the response leaked the minted token"
    );
    let _ = server.stop();
}

/// `--insecure-no-auth` restores the credential-free loopback server: the old
/// local contract, now behind an explicit opt-in.
#[test]
fn insecure_no_auth_keeps_the_credential_free_loopback_server() {
    let server = Server::start(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--insecure-no-auth",
    ]);
    assert_ne!(server.port, 0, "the server must announce a port");
    let body = call(
        "store",
        json!({
            "bank": "anywhere",
            "item": {"id": "insecure-1", "gist": "local paths are unchanged"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    );
    let (status, response) = post(server.port, &body, "");
    assert_eq!(status, 200, "{response}");
    assert!(ok(&response), "{response}");
    // No token was minted, so no token line was printed.
    assert!(
        minted_token(&server.stderr_text()).is_none(),
        "the opt-out must not mint a token: {}",
        server.stderr_text()
    );
    let _ = server.stop();
}
