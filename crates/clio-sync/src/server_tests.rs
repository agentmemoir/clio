// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server transport over real TCP: anonymous-bind
//! refusal, bearer-token auth with no-state-change on rejection, bank
//! authorization, push apply + replay idempotence, commit-ordered pull paging
//! with server-sealed content, and the status surface.
//!
//! ## Owns
//! - Behavior coverage for `server` and the HTTP framing it exposes
//! - Shared raw-socket helpers (`raw_exchange`, `exchange_until`,
//!   `status_request`, `wait_closed`) used by the resource-bound tests too
//!
//! ## Does not own
//! - Serve lifecycle and TTL expiry (`server_lifecycle_tests`)
//! - Apply engine semantics (`apply_tests`)
//! - Client loop behavior (`client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when transport or auth behavior changes; they never change it.
//!
//! Keep this module focused on sync server transport tests.

use crate::credential_fixtures::scoped_store;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation, decode_cursor};
use crate::server::{SyncServer, SyncServerOptions, sync_serve};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store, SyncJournalRow, SyncStore};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn sqlite() -> (Arc<SqliteStore>, Arc<dyn DekProvider>) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    (store, kms)
}

pub(crate) fn serve(
    banks: &[&str],
    token: Option<&str>,
    dev: bool,
    ttl: Option<u64>,
    deadline_secs: Option<u64>,
) -> (SyncServer, Arc<SqliteStore>) {
    let (store, kms) = sqlite();
    // A supplied token is a scoped record in a scratch store, never a
    // single-token server input: `sync_serve` takes only a store file. The
    // guard is kept alive until the store has been loaded at start.
    let scratch = token.map(|secret| scoped_store("serve", secret, banks));
    let server = sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: store.clone(),
        sync: store.clone(),
        kms,
        banks: banks.iter().map(|b| (*b).to_owned()).collect(),
        token_file: scratch.as_ref().map(|s| s.path().to_path_buf()),
        dev_mode: dev,
        ttl_secs: ttl,
        deadline_secs,
        device_id: "srv-1".into(),
    })
    .expect("serve");
    drop(scratch);
    (server, store)
}

fn item(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some(gist.into()),
        valid_from: stamp.into(),
        valid_until: None,
        tx_from: stamp.into(),
        tx_until: None,
        created_at: stamp.into(),
        updated_at: stamp.into(),
        context: None,
    }
}

pub(crate) fn item_value(bank: &str, id: &str, stamp: &str, gist: &str) -> Value {
    serde_json::to_value(item(bank, id, "s1", stamp, gist)).expect("serialize item")
}

pub(crate) fn mutation(mid: &str, bank: &str, stamp: &str, gist: &str) -> SyncMutation {
    let content = item_value(bank, "i-1", stamp, gist);
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: "i-1".into(),
        op: OP_UPSERT.into(),
        transaction_time: stamp.into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(content),
        content_hash: format!("h-{mid}"),
        admission_receipt: None,
    }
}

pub(crate) fn http(
    addr: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |b| serde_json::to_vec(b).expect("serialize body"));
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&payload))
        .expect("send request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = text.find("\r\n\r\n").map_or(text.len(), |p| p + 4);
    let value = serde_json::from_str(text[body_start..].trim()).unwrap_or(Value::Null);
    (status, value)
}

pub(crate) fn push_body(bank: &str, mutations: &[SyncMutation]) -> Value {
    json!({ "device_id": "dev-a", "bank": bank, "mutations": mutations })
}

/// Raw request bytes for one authorized status request.
pub(crate) fn status_request(addr: &str) -> Vec<u8> {
    format!(
        "GET /sync/status HTTP/1.1\r\nHost: {addr}\r\n\
         Authorization: Bearer t1\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

/// One response status parsed from raw response text (`0` = unparseable).
pub(crate) fn status_of(text: &str) -> u16 {
    text.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0)
}

/// Send raw bytes and read the whole response back. Write errors are
/// tolerated (an early refusal can RST mid-write); reads carry a generous
/// client-side timeout so a server hang fails the test instead of hanging CI.
pub(crate) fn raw_exchange(addr: &str, bytes: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("client read timeout");
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read response failed: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = status_of(&text);
    (status, text)
}

/// Poll `raw_exchange` until it returns `want` or the generous deadline
/// expires; each attempt uses a fresh, harmless read-only connection.
pub(crate) fn exchange_until(addr: &str, bytes: &[u8], want: u16, secs: u64) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let (status, text) = raw_exchange(addr, bytes);
        if status == want {
            return (status, text);
        }
        assert!(
            Instant::now() < deadline,
            "never observed {want} within {secs} s; last response was {status}: {text}"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn t24_04_refuses_anonymous_bind_outside_dev_mode() {
    let (store, kms) = sqlite();
    let err = sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: store.clone(),
        sync: store,
        kms,
        banks: vec!["b1".into()],
        token_file: None,
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
    });
    assert!(err.is_err());
}

#[test]
fn t24_04_token_enforced_and_routes_scoped() {
    let (server, store) = serve(&["b1"], Some("s3cret"), false, None, None);
    let addr = server.addr().to_owned();
    let m = mutation("m-1", "b1", "2026-01-01T00:00:00Z", "g");
    let body = push_body("b1", &[m]);
    // Missing and wrong tokens: 401 and nothing applied.
    let (status, _) = http(&addr, "POST", "/sync/push", None, Some(&body));
    assert_eq!(status, 401);
    let (status, _) = http(&addr, "POST", "/sync/push", Some("wrong"), Some(&body));
    assert_eq!(status, 401);
    assert_eq!(store.journal_max_seq("b1").unwrap(), 0);
    assert!(store.get_memory_item("b1", "i-1").unwrap().is_none());
    // Correct token: accepted.
    let (status, _) = http(&addr, "POST", "/sync/push", Some("s3cret"), Some(&body));
    assert_eq!(status, 200);
    assert_eq!(store.journal_max_seq("b1").unwrap(), 1);
    // Unknown route and unauthorized bank.
    let (status, _) = http(&addr, "GET", "/sync/nope", Some("s3cret"), None);
    assert_eq!(status, 404);
    // Push and pull both reject unauthorized banks.
    let (status, value) = http(
        &addr,
        "POST",
        "/sync/push",
        Some("s3cret"),
        Some(&push_body("b2", &[])),
    );
    assert_eq!(status, 403);
    assert_eq!(value["code"], "forbidden");
    let (status, _) = http(
        &addr,
        "POST",
        "/sync/pull",
        Some("s3cret"),
        Some(&json!({ "bank": "b2", "cursor": "", "limit": 1 })),
    );
    assert_eq!(status, 403);
    server.stop();
}

#[test]
fn dev_mode_accepts_anonymous_requests() {
    let (server, _store) = serve(&["b1"], None, true, None, None);
    let empty = push_body("b1", &[]);
    let (status, ack) = http(server.addr(), "POST", "/sync/push", None, Some(&empty));
    assert_eq!(status, 200);
    let accepted = ack["accepted"].as_array().expect("accepted");
    assert!(accepted.is_empty(), "{accepted:?}");
    server.stop();
}

#[test]
fn t24_04_push_applies_replays_noop_rejects_reported() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let m = mutation("m-1", "b1", "2026-01-01T00:00:00Z", "server gist");
    let (status, ack) = http(
        &addr,
        "POST",
        "/sync/push",
        Some("t1"),
        Some(&push_body("b1", &[m])),
    );
    assert_eq!(status, 200);
    assert_eq!(ack["accepted"], json!(["m-1"]));
    assert_eq!(ack["rejected"], json!([]));
    let seq = decode_cursor(ack["cursor"].as_str().expect("cursor"), "b1").expect("decode cursor");
    assert!(seq > 0);
    assert!(store.get_memory_item("b1", "i-1").unwrap().is_some());
    assert_eq!(store.journal_max_seq("b1").unwrap(), 1);
    // Replay: same accepted ids, no new journal rows, no second durable row.
    let replay = mutation("m-1", "b1", "2026-01-01T00:00:00Z", "server gist");
    let (status, ack) = http(
        &addr,
        "POST",
        "/sync/push",
        Some("t1"),
        Some(&push_body("b1", &[replay])),
    );
    assert_eq!(status, 200);
    assert_eq!(ack["accepted"], json!(["m-1"]));
    assert_eq!(store.journal_max_seq("b1").unwrap(), 1);
    // A poison mutation is reported in `rejected`, never silently dropped.
    let poison = mutation("m-poison", "b1", "2026-01-01T00:00:00Z", "g");
    let poison = SyncMutation {
        bank: "b-other".into(),
        ..poison
    };
    let (status, ack) = http(
        &addr,
        "POST",
        "/sync/push",
        Some("t1"),
        Some(&push_body("b1", &[poison])),
    );
    assert_eq!(status, 200);
    assert_eq!(ack["rejected"], json!(["m-poison"]));
    assert_eq!(ack["accepted"], json!([]));
    server.stop();
}

#[test]
fn pull_returns_commit_ordered_pages_and_decrypts_server_seal() {
    let (server, _store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let m1 = mutation("m-1", "b1", "2026-01-01T00:00:00Z", "first");
    let m2 = mutation("m-2", "b1", "2026-01-01T01:00:00Z", "second");
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
    assert_eq!(rows[0]["content"]["data"]["gist"], "first");
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

#[test]
fn status_reports_role_banks_and_dev_flag() {
    let (server, _store) = serve(&["b1", "b2"], Some("t1"), false, None, None);
    let (status, value) = http(server.addr(), "GET", "/sync/status", Some("t1"), None);
    assert_eq!(status, 200);
    assert_eq!(value["role"], "server");
    assert_eq!(value["dev_mode"], json!(false));
    let banks = value["banks"].as_array().expect("banks");
    assert_eq!(banks.len(), 2);
    assert_eq!(banks[0]["bank_id"], "b1");
    assert!(banks[0]["journal_seq"].is_i64());
    assert!(banks[0]["dead_letter_open"].is_i64());
    assert!(banks[0]["conflicts"].is_i64());
    server.stop();
}

pub(crate) fn wait_closed(addr: &str, secs: u64, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while TcpStream::connect(addr).is_ok() {
        assert!(
            Instant::now() < deadline,
            "{label} did not close the listener"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
