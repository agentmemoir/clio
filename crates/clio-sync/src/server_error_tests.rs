// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//
//! This module tests the sync server's error and recovery seams: bind failure,
//! malformed push/pull bodies, malformed mutations, and journal rows whose
//! sealed content cannot be decoded.
//!
//! ## Owns
//! - Error-path coverage for `server` (`sync_serve`, `handle_push`,
//!   `handle_pull`, `journal_row_to_mutation`)
//!
//! ## Does not own
//! - Happy-path transport behavior (`server_tests`)
//! - Client loops (`client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when server error handling changes; they never change it.
//!
//! Keep this module focused on server error-path tests.

use super::server_tests::{http, mutation, push_body, serve, sqlite};
use crate::credential_fixtures::scoped_store;
use crate::server::{SyncServerOptions, sync_serve};
use clio_store::{DekProvider, SyncJournalRow, SyncStore};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;

/// POST a raw (possibly invalid-JSON) body and return the parsed status code.
fn http_raw(addr: &str, path: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer t1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .expect("send");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    text.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// A server on `bank` whose credential store binds `token` to it, plus the
/// store and KMS handles the caller inspects.
fn serve_with_kms(
    bank: &str,
    token: &str,
) -> (
    crate::server::SyncServer,
    std::sync::Arc<clio_store::SqliteStore>,
    std::sync::Arc<dyn DekProvider>,
) {
    let (store, kms) = sqlite();
    let credentials = scoped_store("error-kms", token, &[bank]);
    let server = sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: store.clone(),
        sync: store.clone(),
        kms: kms.clone(),
        banks: vec![bank.to_owned()],
        token_file: Some(credentials.path().to_path_buf()),
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
    })
    .expect("serve");
    (server, store, kms)
}

fn row(id: &str, sealed: Option<String>, encrypted: bool) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: id.into(),
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: "i-1".into(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: "h1".into(),
        content_sealed: sealed,
        client_encrypted: encrypted,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn bind_failure_returns_an_error() {
    let (store, kms) = sqlite();
    // A valid store so the bind is reached; the failure must be the bind.
    let credentials = scoped_store("bind-fail", "t1", &["b1"]);
    let err = sync_serve(SyncServerOptions {
        bind: "not-a-bind-address".into(),
        store: store.clone(),
        sync: store,
        kms,
        banks: vec!["b1".into()],
        token_file: Some(credentials.path().to_path_buf()),
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
    })
    .err()
    .expect("bad bind address");
    assert!(err.message.contains("bind failed"));
}

#[test]
fn malformed_bodies_and_mutations_are_refused() {
    let (server, _store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    // A body that is not JSON at all is refused by the transport.
    assert_eq!(http_raw(&addr, "/sync/push", "not json"), 0);
    assert_eq!(http_raw(&addr, "/sync/pull", "not json"), 0);
    // A well-formed envelope with an unserializable mutation is refused.
    let good = mutation("m-1", "b1", "2026-01-01T00:00:00Z", "g");
    let mut raw = serde_json::to_value(push_body("b1", &[good])).expect("push body");
    raw["mutations"][0] = json!({ "not": "a mutation" });
    let (status, _) = http(&addr, "POST", "/sync/push", Some("t1"), Some(&raw));
    assert_eq!(status, 0);
    // A pull body missing required fields fails request deserialization.
    let (status, _) = http(
        &addr,
        "POST",
        "/sync/pull",
        Some("t1"),
        Some(&json!({ "bank": "b1" })),
    );
    assert_eq!(status, 0);
    server.stop();
}

fn pull_status(addr: &str) -> u16 {
    let req = json!({ "bank": "b1", "cursor": "", "limit": 10 });
    let (status, _) = http(addr, "POST", "/sync/pull", Some("t1"), Some(&req));
    status
}

#[test]
fn corrupt_client_cipher_row_is_a_server_error() {
    let (server, store, _kms) = serve_with_kms("b1", "t1");
    let addr = server.addr().to_owned();
    store
        .journal_append(&row("m-bad-cipher", Some("}{".into()), true))
        .expect("append");
    assert_eq!(pull_status(&addr), 0);
    server.stop();
}

#[test]
fn corrupt_sealed_envelope_is_a_server_error() {
    let (server, store, _kms) = serve_with_kms("b1", "t1");
    let addr = server.addr().to_owned();
    store
        .journal_append(&row("m-bad-seal", Some("not-an-envelope".into()), false))
        .expect("append");
    assert_eq!(pull_status(&addr), 0);
    server.stop();
}

#[test]
fn sealed_content_that_is_not_json_is_a_server_error() {
    let (server, store, kms) = serve_with_kms("b1", "t1");
    let addr = server.addr().to_owned();
    let envelope = kms.encrypt("subj", b"not json").expect("encrypt");
    store
        .journal_append(&row(
            "m-bad-content",
            Some(serde_json::to_string(&envelope).expect("envelope json")),
            false,
        ))
        .expect("append");
    assert_eq!(pull_status(&addr), 0);
    server.stop();
}
