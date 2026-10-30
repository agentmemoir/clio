// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's per-credential bank scoping over real
//! TCP: two clients on one server, the intersection with the allow-list, the
//! filtered status response, the same-length wrong secret, one
//! indistinguishable `401` for every bad credential, restart-scoped
//! revocation, and the fail-closed start on an unusable store.
//!
//! ## Owns
//! - Behavior coverage for the scoped bank checks in `server`
//!
//! ## Does not own
//! - Credential loading and resolution in isolation (`credential_tests`)
//! - The unscoped transport behavior (`server_tests`)
//!
//! ## Boundary
//!
//! Tests here break when scoped authorization changes; they never change it.
//!
//! Keep this module focused on the scoped bank checks in `server`.

use crate::credential_fixtures::{Scratch, record, secret};
use crate::envelope_fixtures::seal_own_content;
use crate::protocol::SyncContent;
use crate::server::server_tests::{http, item_value, mutation, push_body, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve};
use clio_store::{DeadLetterStatus, SqliteStore, Store, SyncStore};
use clio_types::ErrorCode;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

/// Server options bound to `store_path`, and the store handle to inspect what
/// was (or was not) applied. The credential source is the store file itself:
/// there is no single-token option to configure.
fn options(banks: &[&str], store_path: &Path, dev: bool) -> (SyncServerOptions, Arc<SqliteStore>) {
    let (store, kms) = sqlite();
    let options = SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: store.clone(),
        sync: store.clone(),
        kms,
        banks: banks.iter().map(|b| (*b).to_owned()).collect(),
        token_file: Some(store_path.to_path_buf()),
        dev_mode: dev,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
        sync_key: None,
    };
    (options, store)
}

/// A running server bound to `store_path`, with its store handle.
fn serve_file(banks: &[&str], store_path: &Path, dev: bool) -> (SyncServer, Arc<SqliteStore>) {
    let (options, store) = options(banks, store_path, dev);
    (sync_serve(options).expect("serve"), store)
}

#[test]
fn two_clients_on_one_server_are_isolated() {
    let scratch = Scratch::new("two-clients");
    let a = secret("a");
    let b = secret("b");
    scratch.write(&[
        record("client-a", &a, &["b-a"]),
        record("client-b", &b, &["b-b"]),
    ]);
    let (server, store) = serve_file(&["b-a", "b-b"], scratch.path(), false);
    let addr = server.addr().to_owned();

    // Each client reaches its own bank.
    let m = seal_own_content(mutation("m-a", "b-a", "2026-01-01T00:00:00Z", "g"));
    let (status, ack) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-a", &[m])),
    );
    assert_eq!(status, 200);
    assert_eq!(ack["accepted"], json!(["m-a"]));
    // A different item id: ids are store-wide, not per bank.
    let mut m = mutation("m-b", "b-b", "2026-01-01T00:00:00Z", "g");
    m.entity_id = "i-2".into();
    m.content = SyncContent::Plain(item_value("b-b", "i-2", "2026-01-01T00:00:00Z", "g"));
    let m = seal_own_content(m);
    let (status, value) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(b.as_str()),
        Some(&push_body("b-b", &[m])),
    );
    assert_eq!(status, 200, "{value}");

    // Neither reaches the other's bank, in either direction, and nothing on
    // the refused bank moves.
    for (token, bank) in [(a.as_str(), "b-b"), (b.as_str(), "b-a")] {
        let (status, value) = http(
            &addr,
            "POST",
            "/sync/push",
            Some(token),
            Some(&push_body(bank, &[])),
        );
        assert_eq!(status, 403, "push {bank}: {value}");
        assert_eq!(value["code"], "forbidden");
        let (status, value) = http(
            &addr,
            "POST",
            "/sync/pull",
            Some(token),
            Some(&json!({ "bank": bank, "cursor": "", "limit": 1 })),
        );
        assert_eq!(status, 403, "pull {bank}: {value}");
        assert_eq!(value["code"], "forbidden");
    }
    assert_eq!(store.journal_max_seq("b-a").unwrap(), 1);
    assert_eq!(store.journal_max_seq("b-b").unwrap(), 1);

    // Status lists each credential only its own bank, with the response keys
    // unchanged.
    for (token, bank) in [(a.as_str(), "b-a"), (b.as_str(), "b-b")] {
        let (status, value) = http(&addr, "GET", "/sync/status", Some(token), None);
        assert_eq!(status, 200);
        assert_eq!(value["role"], "server");
        assert!(value["device_id"].is_string());
        assert!(value["dev_mode"].is_boolean());
        assert!(value["timestamp"].is_string());
        let banks = value["banks"].as_array().expect("banks");
        assert_eq!(banks.len(), 1, "only one bank may be visible: {value}");
        assert_eq!(banks[0]["bank_id"], bank);
    }
    server.stop();
}

#[test]
fn a_refused_mutation_is_quarantined_under_the_authorized_bank() {
    let scratch = Scratch::new("quarantine");
    let a = secret("a");
    scratch.write(&[record("client-a", &a, &["b-a"])]);
    let (server, store) = serve_file(&["b-a", "b-b"], scratch.path(), false);
    let addr = server.addr().to_owned();

    // The envelope names the allowed bank; the mutation targets another one.
    let m = mutation("m-x", "b-b", "2026-01-01T00:00:00Z", "g");
    let (status, ack) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-a", &[m])),
    );
    assert_eq!(status, 200);
    assert_eq!(ack["accepted"], json!([]));
    assert_eq!(ack["rejected"], json!(["m-x"]));
    assert_eq!(store.journal_max_seq("b-b").unwrap(), 0);
    // The refusal writes nothing into the refused bank: the row lands under
    // the envelope's bank, the one the credential was authorized for.
    assert_eq!(
        store
            .dead_letter_count("b-b", DeadLetterStatus::Open)
            .unwrap(),
        0,
        "a refusal never writes into the bank it refused"
    );
    assert_eq!(
        store
            .dead_letter_count("b-a", DeadLetterStatus::Open)
            .unwrap(),
        1,
        "the refused row stays visible under the authorized bank"
    );
    server.stop();
}

#[test]
fn a_bank_outside_the_allow_list_is_refused_for_a_scoped_record() {
    let scratch = Scratch::new("intersection");
    let a = secret("a");
    // The record names both banks; the server carries only the first.
    scratch.write(&[record("client-a", &a, &["b-a", "b-b"])]);
    let (server, store) = serve_file(&["b-a"], scratch.path(), false);
    let addr = server.addr().to_owned();

    let (status, value) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-b", &[])),
    );
    assert_eq!(status, 403, "{value}");
    assert_eq!(store.journal_max_seq("b-b").unwrap(), 0);
    // Status narrows to the intersection too: b-b is outside the allow-list.
    let (status, value) = http(&addr, "GET", "/sync/status", Some(a.as_str()), None);
    assert_eq!(status, 200);
    assert_eq!(value["banks"].as_array().expect("banks").len(), 1);
    server.stop();
}

#[test]
fn a_bank_outside_the_record_is_refused_with_no_fallback_to_the_allow_list() {
    let scratch = Scratch::new("no-fallback");
    let a = secret("a");
    scratch.write(&[record("client-a", &a, &["b-a"])]);
    // The server carries both banks; only the record's list may be reached.
    let (server, store) = serve_file(&["b-a", "b-b"], scratch.path(), false);
    let addr = server.addr().to_owned();

    let m = mutation("m-x", "b-b", "2026-01-01T00:00:00Z", "g");
    let (status, value) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-b", &[m])),
    );
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "forbidden");
    assert_eq!(
        store.journal_max_seq("b-b").unwrap(),
        0,
        "the refusal must not be re-targeted at the refused bank"
    );
    // The allowed bank still works: the refusal fell back to nothing.
    let (status, _) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-a", &[])),
    );
    assert_eq!(status, 200);
    server.stop();
}

#[test]
fn an_unknown_or_same_length_secret_is_401_and_changes_nothing() {
    let scratch = Scratch::new("same-length");
    let a = secret("a");
    let wrong = secret("b");
    assert_eq!(a.len(), wrong.len(), "fixture must be same-length");
    scratch.write(&[record("client-a", &a, &["b-a"])]);
    let (server, store) = serve_file(&["b-a"], scratch.path(), false);
    let addr = server.addr().to_owned();

    // A missing header, an unknown secret, and a same-length wrong secret all
    // answer the one fixed 401 body, so a refusal never discloses which
    // credentials exist or which of the two the request got wrong.
    let fixed = json!({ "ok": false, "code": "forbidden", "message": "unauthorized" });
    let (missing, missing_body) = http(&addr, "GET", "/sync/status", None, None);
    assert_eq!(missing, 401);
    assert_eq!(missing_body, fixed);
    for secret in [wrong.as_str(), "never-stored", ""] {
        let m = mutation("m-x", "b-a", "2026-01-01T00:00:00Z", "g");
        let (status, body) = http(
            &addr,
            "POST",
            "/sync/push",
            Some(secret),
            Some(&push_body("b-a", &[m])),
        );
        assert_eq!(status, 401);
        assert_eq!(
            body, fixed,
            "an unknown or malformed secret must not be distinguishable from a missing one"
        );
        let (status, body) = http(&addr, "GET", "/sync/status", Some(secret), None);
        assert_eq!(status, 401);
        assert_eq!(body, fixed);
    }
    assert_eq!(store.journal_max_seq("b-a").unwrap(), 0);
    assert!(store.get_memory_item("b-a", "i-1").unwrap().is_none());
    server.stop();
}

#[test]
fn a_configured_store_disables_the_dev_anonymous_fallback() {
    let scratch = Scratch::new("dev");
    let a = secret("a");
    scratch.write(&[record("client-a", &a, &["b-a"])]);
    let (server, _store) = serve_file(&["b-a"], scratch.path(), true);
    let addr = server.addr().to_owned();

    let (status, _) = http(
        &addr,
        "POST",
        "/sync/push",
        None,
        Some(&push_body("b-a", &[])),
    );
    assert_eq!(
        status, 401,
        "a configured store must require credentials even in dev mode"
    );
    let (status, _) = http(
        &addr,
        "POST",
        "/sync/push",
        Some(a.as_str()),
        Some(&push_body("b-a", &[])),
    );
    assert_eq!(status, 200);
    server.stop();
}

#[test]
fn a_removed_credential_is_refused_after_the_next_start() {
    let scratch = Scratch::new("removed");
    let a = secret("a");
    let b = secret("b");
    scratch.write(&[
        record("client-a", &a, &["b-a"]),
        record("client-b", &b, &["b-a"]),
    ]);
    let (server, _store) = serve_file(&["b-a"], scratch.path(), false);
    let (status, _) = http(server.addr(), "GET", "/sync/status", Some(a.as_str()), None);
    assert_eq!(status, 200);
    server.stop();

    // The operator removes record a and starts again.
    scratch.write(&[record("client-b", &b, &["b-a"])]);
    let (server, _store) = serve_file(&["b-a"], scratch.path(), false);
    let (status, _) = http(server.addr(), "GET", "/sync/status", Some(a.as_str()), None);
    assert_eq!(status, 401, "revocation takes effect at the next start");
    let (status, _) = http(server.addr(), "GET", "/sync/status", Some(b.as_str()), None);
    assert_eq!(status, 200);
    server.stop();
}

#[test]
fn an_unusable_store_refuses_the_start() {
    let absent = Scratch::new("start-absent");
    let (opts, _store) = options(&["b-a"], absent.path(), false);
    let err = sync_serve(opts).err().expect("an absent store is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);

    let empty = Scratch::new("start-empty");
    empty.write(&[]);
    let (opts, _store) = options(&["b-a"], empty.path(), false);
    let err = sync_serve(opts).err().expect("an empty store is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}
