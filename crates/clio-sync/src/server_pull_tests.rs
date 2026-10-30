// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's pull route over real TCP: commit-ordered
//! paging with the pusher's own seal, resume cursors, and a journal row that
//! carries no sealed content.
//!
//! ## Owns
//! - Behavior coverage for the pull route's paging, cursors, and served content
//!
//! ## Does not own
//! - The pull-side denial refusal and the bank guards (`server_denial_tests`)
//! - The push route and the status surface (`server_tests`)
//! - Serve lifecycle and TTL expiry (`server_lifecycle_tests`)
//!
//! ## Boundary
//!
//! Tests here break when pull paging or served content changes; they never
//! change it.
//!
//! Keep this module focused on the pull route's transport tests.

use crate::envelope_fixtures::{opened_document, seal_own_content};
use crate::protocol::decode_cursor;
use crate::server::server_tests::{http, mutation, push_body, serve};
use clio_store::{SyncJournalRow, SyncStore};
use serde_json::json;

#[test]
fn pull_returns_commit_ordered_pages_and_decrypts_server_seal() {
    let (server, _store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let m1 = seal_own_content(mutation("m-1", "b1", "2026-01-01T00:00:00Z", "first"));
    let m2 = seal_own_content(mutation("m-2", "b1", "2026-01-01T01:00:00Z", "second"));
    let (status, _) = http(
        &addr,
        "POST",
        "/sync/push",
        Some("t1"),
        Some(&push_body("b1", &[m1, m2])),
    );
    assert_eq!(status, 200);
    let req = json!({ "bank": "b1", "cursor": "", "limit": 1 });
    let (status, page) = http(&addr, "POST", "/sync/pull", Some("t1"), Some(&req));
    assert_eq!(status, 200);
    let rows = page["mutations"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sync_mutation_id"], "m-1");
    // The page carries the pusher's own seal, which a blind relay never read.
    assert_eq!(opened_document(&rows[0])["gist"], "first");
    assert_eq!(page["has_more"], json!(true));
    let next = page["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_owned();
    assert_eq!(decode_cursor(&next, "b1").expect("decode"), 1);
    let req = json!({ "bank": "b1", "cursor": next, "limit": 1 });
    let (status, page) = http(&addr, "POST", "/sync/pull", Some("t1"), Some(&req));
    assert_eq!(status, 200);
    let rows = page["mutations"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sync_mutation_id"], "m-2");
    // has_more is `rows.len() >= limit`, so a full page reports true.
    assert_eq!(page["has_more"], json!(true));
    // Past-the-end page repeats the resume cursor with no rows.
    let next = page["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_owned();
    let req = json!({ "bank": "b1", "cursor": next, "limit": 1 });
    let (status, page) = http(&addr, "POST", "/sync/pull", Some("t1"), Some(&req));
    assert_eq!(status, 200);
    let rows = page["mutations"].as_array().expect("rows");
    assert!(rows.is_empty(), "{rows:?}");
    server.stop();
}

#[test]
fn pull_survives_unsealed_journal_row_without_http_500() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    // A KMS-less writer journals routing metadata with no sealed content.
    store
        .journal_append(&SyncJournalRow {
            seq: 0,
            sync_mutation_id: "m-unsealed".into(),
            bank_id: "b1".into(),
            entity_kind: "item".into(),
            entity_id: "i-1".into(),
            op: "upsert".into(),
            transaction_time: "2026-01-01T00:00:00Z".into(),
            device_id: "dev-a".into(),
            content_hash: "h1".into(),
            content_sealed: None,
            client_encrypted: false,
            admission_receipt: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        })
        .expect("append unsealed");
    let req = json!({ "bank": "b1", "cursor": "", "limit": 10 });
    let (status, page) = http(&addr, "POST", "/sync/pull", Some("t1"), Some(&req));
    assert_eq!(status, 200, "unsealed row must not 500 the pull: {page}");
    let rows = page["mutations"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sync_mutation_id"], "m-unsealed");
    assert_eq!(rows[0]["content"]["mode"], "plain");
    assert_eq!(rows[0]["content"]["data"], json!({}));
    server.stop();
}
