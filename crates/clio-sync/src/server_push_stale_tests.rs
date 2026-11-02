// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the push route's refusal of a stale envelope: the ack
//! reports it rejected instead of accepted, the row is dead-lettered under its
//! own reason code, the bank's refusal count moves, and the last refusal is
//! readable in the node-wide status slot.
//!
//! ## Owns
//! - Behavior coverage for a stale push's ack, dead letter, and counter
//! - Behavior coverage for the sender holding its push cursor on a refusal
//! - Behavior coverage for the `__status` last-refusal line
//!
//! ## Does not own
//! - The freshness decision itself (`apply_freshness_tests`)
//! - The pull path's stale handling (`client_freshness_tests`)
//!
//! ## Boundary
//!
//! These tests break when a stale push's acknowledgement or its operator
//! record changes; they never change either.
//!
//! Keep this module focused on the stale push refusal.

use crate::apply_fixtures::{env, item_value};
use crate::client::client_tests::{TOKEN, cfg, create_item, paired_cfg, paired_key};
use crate::client::{push as client_push, sync_status};
use crate::credential_fixtures::scoped_store;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, REASON_STALE_ENVELOPE, SyncMutation};
use crate::server::server_tests::{http, push_body, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve};
use clio_store::{DeadLetterStatus, SqliteStore, Store, SyncStore};
use serde_json::{Value, json};
use std::sync::Arc;

/// The window every server in this module applies, in seconds.
const WINDOW: u64 = 300;

/// A stamp far enough in the past that any window refuses it.
const OLD_STAMP: &str = "2026-01-01T00:00:00Z";

/// A running server whose freshness window is on, because the shipped default
/// leaves it off and would never refuse a row.
fn serve_with_window(bank: &str) -> (SyncServer, Arc<SqliteStore>) {
    let scratch = scoped_store("push-stale", TOKEN, &[bank]);
    let (store, kms) = sqlite();
    let server = sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: Arc::clone(&store) as Arc<dyn SyncStore>,
        kms,
        banks: vec![bank.into()],
        token_file: Some(scratch.path().to_path_buf()),
        dev_mode: false,
        allow_public_bind: false,
        ttl_secs: None,
        deadline_secs: None,
        max_envelope_skew_secs: Some(WINDOW),
        device_id: "srv-1".into(),
        sync_key: None,
    })
    .expect("serve");
    (server, store)
}

/// One item mutation sealed under `key`, the only content shape the push route
/// accepts from a peer.
fn sealed(mid: &str, bank: &str, id: &str, stamp: &str) -> SyncMutation {
    let content = item_value(bank, id, &format!("s-{id}"), stamp, "sealed gist");
    let mutation = env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        id,
        OP_UPSERT,
        stamp,
        "dev-a",
        content.clone(),
    );
    crate::envelope_fixtures::seal_content_for(mutation, &paired_key(), &content)
}

/// One push of `mutations` under `bank` with the module's token.
fn push(addr: &str, bank: &str, mutations: &[SyncMutation]) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/push",
        Some(TOKEN),
        Some(&push_body(bank, mutations)),
    )
}

/// A stale push is refused by name: the ack rejects it, and the row is
/// quarantined under its own reason code instead of vanishing.
#[test]
fn a_stale_push_is_rejected_and_quarantined() {
    let (server, store) = serve_with_window("b-a");
    let addr = server.addr().to_owned();
    let stale = sealed("m-stale", "b-a", "i-stale", OLD_STAMP);
    let (status, ack) = push(&addr, "b-a", std::slice::from_ref(&stale));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(
        ack["accepted"],
        json!([]),
        "a stale row is not acknowledged as accepted: {ack}"
    );
    assert_eq!(
        ack["rejected"],
        json!(["m-stale"]),
        "the sender is told the row did not land: {ack}"
    );
    assert!(
        store.journal_get("b-a", "m-stale").unwrap().is_none(),
        "a stale row is never journaled"
    );
    assert!(
        store.get_memory_item("b-a", "i-stale").unwrap().is_none(),
        "a stale row writes no content"
    );
    let rows = store
        .dead_letter_list("b-a", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(rows.len(), 1, "the refusal is inspectable");
    assert_eq!(rows[0].reason_code, REASON_STALE_ENVELOPE);
    assert!(
        rows[0].detail.contains(OLD_STAMP),
        "the detail names the refused stamp: {}",
        rows[0].detail
    );
    server.stop();
}

/// The sender holds its push cursor on the refusal: the ack is the only thing
/// that tells it the row is not stored, so it must answer rejected.
#[test]
fn a_stale_push_holds_the_senders_push_cursor() {
    let (server, _store) = serve_with_window("b-a");
    let addr = server.addr().to_owned();
    let client_store = crate::client::client_tests::sqlite();
    create_item(&client_store, "b-a", "i-old", "old gist");
    let cfg = paired_cfg("dev-a", &client_store, &addr);
    let rows = client_push(&cfg, &["b-a".to_owned()]).expect("push runs");
    assert_eq!(rows.len(), 1, "one bank reported");
    assert_eq!(
        cfg.sync.cursor_get("b-a", "push").unwrap(),
        None,
        "a refused row must not advance the sender as if it landed"
    );
    assert!(
        rows[0].pending_push > 0,
        "the row stays pending on the sender: {}",
        rows[0].pending_push
    );
    server.stop();
}

/// A fresh push applies and leaves the counter alone; only a refusal counts,
/// and the count only grows.
#[test]
fn stale_refused_counts_refusals_and_never_an_applied_row() {
    let (server, store) = serve_with_window("b-a");
    let addr = server.addr().to_owned();
    let fresh = sealed("m-fresh", "b-a", "i-fresh", &crate::clock::now_utc());
    let (_, ack) = push(&addr, "b-a", std::slice::from_ref(&fresh));
    assert_eq!(ack["accepted"], json!(["m-fresh"]), "{ack}");
    assert_eq!(
        crate::apply::stale_refused(store.as_ref(), "b-a").unwrap(),
        0,
        "an applied row is not a refusal"
    );
    let stale = sealed("m-stale", "b-a", "i-stale", OLD_STAMP);
    let (_, ack) = push(&addr, "b-a", std::slice::from_ref(&stale));
    assert_eq!(ack["rejected"], json!(["m-stale"]), "{ack}");
    assert_eq!(
        crate::apply::stale_refused(store.as_ref(), "b-a").unwrap(),
        1,
        "the refusal is counted"
    );
    let another = sealed("m-stale-2", "b-a", "i-stale-2", OLD_STAMP);
    let (_, ack) = push(&addr, "b-a", std::slice::from_ref(&another));
    assert_eq!(ack["rejected"], json!(["m-stale-2"]), "{ack}");
    assert_eq!(
        crate::apply::stale_refused(store.as_ref(), "b-a").unwrap(),
        2,
        "the counter is monotonic"
    );
    let later = sealed("m-fresh-2", "b-a", "i-fresh-2", &crate::clock::now_utc());
    let (_, ack) = push(&addr, "b-a", std::slice::from_ref(&later));
    assert_eq!(ack["accepted"], json!(["m-fresh-2"]), "{ack}");
    assert_eq!(
        crate::apply::stale_refused(store.as_ref(), "b-a").unwrap(),
        2,
        "a later applied row does not reset or move the counter"
    );
    server.stop();
}

/// The last refusal lands in the status slot beside `last_error`, and the
/// status report surfaces it with the per-bank count.
#[test]
fn the_last_stale_refusal_is_visible_in_the_status_slot() {
    let (server, store) = serve_with_window("b-a");
    let addr = server.addr().to_owned();
    let stale = sealed("m-stale", "b-a", "i-stale", OLD_STAMP);
    let (_, ack) = push(&addr, "b-a", std::slice::from_ref(&stale));
    assert_eq!(ack["rejected"], json!(["m-stale"]), "{ack}");
    let line = store
        .cursor_get("__status", "last_stale_refusal")
        .unwrap()
        .expect("the refusal is recorded in the status slot");
    assert!(line.contains(REASON_STALE_ENVELOPE), "{line}");
    assert!(line.contains("m-stale"), "{line}");
    assert!(line.contains(OLD_STAMP), "{line}");

    let report_cfg = cfg("srv-1", &store, "http://status.invalid", Vec::new());
    let report = sync_status(&report_cfg, &["b-a".to_owned()]).expect("status runs");
    assert_eq!(
        report.banks[0].stale_refused, 1,
        "the report carries the counter"
    );
    assert_eq!(
        report.last_stale_refusal.as_deref(),
        Some(line.as_str()),
        "the report carries the last refusal"
    );
    server.stop();
}
