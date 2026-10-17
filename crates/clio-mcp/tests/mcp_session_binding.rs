// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for session binding over the real
//! credential-bound HTTP server: a session minted by one principal serves that
//! principal, and is refused like an unknown id for anyone else, at reuse and
//! at `DELETE`.
//!
//! The pinned transport revision answers a terminated or unknown session id
//! with 404, so a cross-principal presentation carries no extra signal. An
//! unbound server keeps the lifecycle-only behavior it always had.
//!
//! Run locally with `cargo nextest run -p clio-mcp --test mcp_session_binding`.
//!
//! The harness lives in `mcp_principal_binding_support`.
//!
//! ## Owns
//! - The bound server setups this target drives: two store principals, and one
//!   unbound loopback server
//! - The raw `POST`, `DELETE`, and `initialize` probes, and the mint helper
//!   that returns the id the server assigned
//! - Owner reuse, cross-principal reuse, unknown id, owner-only delete, and the
//!   unchanged unbound lifecycle
//!
//! ## Does not own
//! - The harness itself (`mcp_principal_binding_support`), which is shared with
//!   the sibling credential-binding target
//! - The registry-level predicate and the guards themselves
//!   (`clio-mcp`'s `http_guard` tests)
//! - The bind-time refusals and the default loopback bind (`http_tests`)
//!
//! ## Boundary
//!
//! Changes to what a bound server answers for a presented session id should
//! normally require changes here. Changes to start-up refusals or to the guards
//! in isolation should not require changes here.
//!
//! Keep this module focused on the session lifecycle over a served socket.

// The harness is shared with the sibling credential-binding target and is
// compiled into this one whole, so this target uses only part of it.
#[allow(dead_code)]
mod mcp_principal_binding_support;

use clio_mcp::http::HttpConfig;
use clio_mcp::http_auth::CredentialStore;
use clio_mcp::runtime::McpOpenOptions;
use clio_types::Actor;
use mcp_principal_binding_support::{Store, bearer, spawn_server_with};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

/// A bound server with sessions enabled: two principals, one bank.
fn bound_sessions_server(store: &CredentialStore) -> u16 {
    spawn_server_with(
        HttpConfig {
            credentials: Some(Arc::new(store.clone())),
            sessions_enabled: true,
            ..HttpConfig::default()
        },
        &McpOpenOptions {
            default_bank: "demo".into(),
            ..McpOpenOptions::default()
        },
    )
}

/// An unbound loopback server with sessions enabled, exactly as a dev server.
/// The harness opts out of the start-time auth requirement, the way the
/// `--insecure-no-auth` flag does, so the unbound session contract is exercised.
fn unbound_sessions_server() -> u16 {
    spawn_server_with(
        HttpConfig {
            sessions_enabled: true,
            insecure_no_auth: true,
            ..HttpConfig::default()
        },
        &McpOpenOptions {
            default_bank: "demo".into(),
            ..McpOpenOptions::default()
        },
    )
}

/// One `POST /mcp`, returning the status, the `MCP-Session-Id` header, and the
/// body. The harness `post` reads only the body, and the minted id travels in
/// a header, so this variant exists where the id matters.
fn post_with_session(port: u16, body: &str, extra: &str) -> (u16, Option<String>, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos.min(buf.len())]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let session = head.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.eq_ignore_ascii_case("mcp-session-id")
            .then(|| v.trim().to_owned())
    });
    let from = (pos + 4).min(buf.len());
    (
        status,
        session,
        String::from_utf8_lossy(&buf[from..]).into_owned(),
    )
}

/// One `DELETE /mcp` carrying `session`, answered with its status.
fn delete_session(port: u16, session: &str, extra: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request = format!(
        "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\n{extra}MCP-Session-Id: {session}\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).expect("write");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.read_to_end(&mut buf);
    let head = String::from_utf8_lossy(&buf);
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Mint a session as `secret`, returning the id the server assigned.
fn mint(port: u16, secret: &str) -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}
    })
    .to_string();
    let (status, session, body) = post_with_session(port, &init, &bearer(secret));
    assert_eq!(status, 200, "{body}");
    session.unwrap_or_else(|| panic!("initialize must mint a session: {body}"))
}

/// A `tools/list` frame, the cheapest call the session gate guards.
fn list() -> String {
    json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string()
}

#[test]
fn a_session_minted_by_one_principal_is_reused_by_the_same_principal() {
    let store = Store::new("same-owner");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_sessions_server(&credentials);

    let session = mint(port, &secrets[0]);
    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!("{}MCP-Session-Id: {session}\r\n", bearer(&secrets[0])),
    );
    assert_eq!(status, 200, "the owner must reuse the session: {body}");
}

#[test]
fn the_same_session_presented_by_another_principal_is_404() {
    let store = Store::new("cross-principal");
    let mut secrets = Vec::new();
    let credentials = store.write(
        &[
            ("ci", Actor::Harness, &["demo"]),
            ("bot", Actor::Agent, &["demo"]),
        ],
        &mut secrets,
    );
    let port = bound_sessions_server(&credentials);

    let session = mint(port, &secrets[0]);
    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!("{}MCP-Session-Id: {session}\r\n", bearer(&secrets[1])),
    );
    assert_eq!(status, 404, "a foreign principal must be refused: {body}");

    // The refusal removed nothing: the owner's session is still live.
    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!("{}MCP-Session-Id: {session}\r\n", bearer(&secrets[0])),
    );
    assert_eq!(status, 200, "the owner's session must survive: {body}");
}

#[test]
fn an_unknown_session_id_is_still_404() {
    let store = Store::new("unknown-id");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_sessions_server(&credentials);

    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!(
            "{}MCP-Session-Id: sess-0000000000000000\r\n",
            bearer(&secrets[0])
        ),
    );
    assert_eq!(status, 404, "{body}");
}

#[test]
fn delete_by_the_owner_ends_the_session_and_a_foreign_delete_is_404() {
    let store = Store::new("delete-owner");
    let mut secrets = Vec::new();
    let credentials = store.write(
        &[
            ("ci", Actor::Harness, &["demo"]),
            ("bot", Actor::Agent, &["demo"]),
        ],
        &mut secrets,
    );
    let port = bound_sessions_server(&credentials);

    let session = mint(port, &secrets[0]);
    let foreign = delete_session(port, &session, &bearer(&secrets[1]));
    assert_eq!(foreign, 404, "a foreign delete must be refused");
    // The foreign delete removed nothing.
    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!("{}MCP-Session-Id: {session}\r\n", bearer(&secrets[0])),
    );
    assert_eq!(
        status, 200,
        "the session must survive a foreign delete: {body}"
    );

    let owner = delete_session(port, &session, &bearer(&secrets[0]));
    assert_eq!(owner, 200, "the owner's delete must remove it");
    let (status, _, body) = post_with_session(
        port,
        &list(),
        &format!("{}MCP-Session-Id: {session}\r\n", bearer(&secrets[0])),
    );
    assert_eq!(status, 404, "the session must be gone: {body}");
}

#[test]
fn an_unbound_server_with_sessions_keeps_the_lifecycle() {
    let port = unbound_sessions_server();

    let session = mint(port, "");
    let (status, _, body) =
        post_with_session(port, &list(), &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(status, 200, "reuse without any credential: {body}");
    assert_eq!(delete_session(port, &session, ""), 200, "delete");
    let (status, _, body) =
        post_with_session(port, &list(), &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(status, 404, "the session must be gone: {body}");
}
