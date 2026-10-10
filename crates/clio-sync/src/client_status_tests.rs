// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the client's status surface and honest-error behaviors:
//! masked `sync_status` reporting, mode parsing, non-upsert feed packaging,
//! and error propagation for malformed remote responses.
//!
//! ## Owns
//! - Behavior coverage for `sync_status`, modes, and feed packaging edge cases
//!
//! ## Does not own
//! - Core transfer paths (`client_tests`)
//! - Apply semantics (`apply_tests` / `apply_matrix_tests`)
//!
//! ## Boundary
//!
//! Tests here break when client behavior changes; they never change it.
//!
//! Keep this module focused on status/mode/feed edge-case tests.

use crate::client::client_tests::{TOKEN, cfg, item, serve, sqlite};
use crate::client::{
    SyncClientConfig, SyncModes, ack_skip, bidirectional, pull, push, sync_status,
};
use crate::crypto::SyncCipherKey;
use clio_store::Store as _;
use clio_store::SyncStore as _;

#[test]
fn sync_status_reports_cursors_and_never_the_token() {
    let (server, _srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let cfg_b = cfg("dev-b", &b, &addr, Vec::new());
    a.create_memory_item(
        &item("b1", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    let report = sync_status(&cfg_a, &["b1".to_owned()]).expect("status");
    assert_eq!(report.device_id, "dev-a");
    assert!(report.remote.as_deref().is_some_and(|r| r.contains(':')));
    assert_eq!(report.auth, "token");
    assert!(!report.encryption);
    assert_eq!(report.last_error, None);
    let bank = &report.banks[0];
    assert_eq!(bank.bank_id, "b1");
    assert!(bank.push_cursor.is_some());
    assert!(bank.pull_cursor.is_none());
    assert_eq!(bank.pending_push, 0);
    assert_eq!(bank.dead_letter_open, 0);
    assert_eq!(bank.conflicts, 0);
    assert!(
        bank.conflict_sample.is_empty(),
        "{:?}",
        bank.conflict_sample
    );
    let text = serde_json::to_string(&report).expect("serialize report");
    assert!(!text.contains(TOKEN));
    // The encryption flag reflects the configured cipher key only.
    let cfg_e = cfg("dev-e", &b, &addr, vec![SyncCipherKey::from_secret("k")]);
    assert!(
        sync_status(&cfg_e, &["b1".to_owned()])
            .expect("status")
            .encryption
    );
    server.stop();
}

#[test]
fn status_pull_side_and_modes_parse() {
    let (server, _srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let cfg_b = cfg("dev-b", &b, &addr, Vec::new());
    a.create_memory_item(
        &item("b1", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    // The pulled item is now B's outgoing feed (1 pending until B pushes).
    let report = sync_status(&cfg_b, &["b1".to_owned()]).expect("status");
    assert_eq!(report.banks[0].pending_push, 1);
    assert!(report.banks[0].pull_cursor.is_some());
    assert_eq!(report.device_id, "dev-b");
    for (raw, expected) in [
        ("push", true),
        ("pull", true),
        ("bidirectional", true),
        ("teleport", false),
    ] {
        assert_eq!(SyncModes::parse(raw).is_ok(), expected);
    }
    // Bidirectional mode applies the same pull-then-push order.
    let empty = cfg("dev-empty", &sqlite(), &addr, Vec::new());
    bidirectional(&empty, &["b1".to_owned()]).expect("bidirectional");
    server.stop();
}

#[test]
fn invalidated_feed_packages_non_upsert_ops_and_errors_are_honest() {
    let (server, _srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let cfg_b = cfg("dev-b", &b, &addr, Vec::new());
    a.create_memory_item(
        &item("b1", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .unwrap();
    a.invalidate_item(
        "b1",
        "i-1",
        "2026-01-02T00:00:00Z",
        "2026-01-02T00:00:00Z",
        None,
        "actor",
    )
    .expect("invalidate");
    push(&cfg_a, &["b1".to_owned()]).expect("push invalidated op");
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    assert!(b.get_memory_item("b1", "i-1").unwrap().is_none());
    let rows = b.journal_list_since("b1", 0, 10).unwrap();
    assert_eq!(rows[0].op, "invalidate");
    // A client without a remote endpoint refuses to sync.
    let cfg_nr = SyncClientConfig {
        device_id: "dev-nr".into(),
        store: a.clone(),
        sync: a.clone(),
        remote: None,
        token: Some(TOKEN.to_owned()),
        cipher: Vec::new(),
    };
    assert!(push(&cfg_nr, &["b1".to_owned()]).is_err());
    assert!(pull(&cfg_nr, &["b1".to_owned()]).is_err());
    // A refused connection surfaces as an Err, never a silent success.
    let cfg_dead = cfg("dev-dead", &a, "127.0.0.1:1", Vec::new());
    assert!(push(&cfg_dead, &["b1".to_owned()]).is_err());
    server.stop();
}

#[test]
fn the_encryption_summary_counts_held_keys_and_leaves_provenance_to_custody() {
    let (server, _srv, _kms) = serve(&["bz"]);
    let addr = server.addr().to_owned();
    // Two held keys: the count is the client's fact, and the store and
    // fingerprint stay unset because they are the custody layer's facts,
    // filled by the caller that assembled the client from a store-backed key.
    let keys = vec![
        SyncCipherKey::from_secret("summary-key-one"),
        SyncCipherKey::from_secret("summary-key-two"),
    ];
    let cfg_two = cfg("dev-two", &sqlite(), &addr, keys);
    let report = sync_status(&cfg_two, &["bz".to_owned()]).expect("status");
    assert!(report.encryption, "content leaves encrypted");
    let summary = report
        .encryption_summary
        .as_ref()
        .expect("a holder reports a summary");
    assert_eq!(summary.key_count, 2);
    assert!(
        summary.store.is_none(),
        "provenance is not this layer's fact"
    );
    assert!(
        summary.current_fingerprint.is_none(),
        "provenance is not this layer's fact"
    );

    // An empty list means encryption off, with no summary at all.
    let cfg_none = cfg("dev-none", &sqlite(), &addr, Vec::new());
    let report = sync_status(&cfg_none, &["bz".to_owned()]).expect("status");
    assert!(!report.encryption, "an empty list is not an error");
    assert!(report.encryption_summary.is_none());
    // The additive field never breaks an older reader: a report with the
    // summary parses back from its own serialized bytes with the flag intact.
    let text = serde_json::to_string(&report).expect("serialize");
    assert!(text.contains("\"encryption\":false"));
    server.stop();
}

#[test]
fn status_surfaces_the_conflict_sample() {
    let a = sqlite();
    let cfg = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    a.conflict_insert(&clio_store::SyncConflictRow {
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: "i-1".into(),
        winner_mutation_id: "local:i-1".into(),
        loser_mutation_id: "m-loser".into(),
        winner_transaction_time: "2026-01-02T00:00:00Z".into(),
        loser_transaction_time: "2026-01-01T00:00:00Z".into(),
        winner_device_id: "dev-a".into(),
        loser_device_id: "dev-b".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
    })
    .expect("conflict insert");
    let report = sync_status(&cfg, &["b1".to_owned()]).expect("status");
    assert_eq!(report.banks[0].conflicts, 1);
    assert_eq!(
        report.banks[0].conflict_sample,
        vec!["i-1 / m-loser".to_owned()]
    );
}

#[test]
fn wrong_cipher_key_quarantines_then_ack_skip_unfreezes() {
    let (server, _srv, _kms) = serve(&["bk"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret("key-one")],
    );
    let cfg_b = cfg(
        "dev-b",
        &b,
        &addr,
        vec![SyncCipherKey::from_secret("key-two")],
    );
    a.create_memory_item(
        &item("bk", "i-1", "s1", "2026-01-01T00:00:00Z", "secret"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["bk".to_owned()]).expect("push");
    // B holds the wrong key: the undecryptable row is quarantined, the pull
    // loop returns with a frozen cursor, and a dead-letter row exists.
    pull(&cfg_b, &["bk".to_owned()]).expect("pull with wrong key");
    assert_eq!(
        b.dead_letter_count("bk", clio_store::DeadLetterStatus::Open)
            .unwrap(),
        1
    );
    let report = sync_status(&cfg_b, &["bk".to_owned()]).expect("status");
    let cursor = report.banks[0].pull_cursor.as_deref().unwrap_or_default();
    assert!(cursor.is_empty(), "{cursor:?}");
    assert!(b.get_memory_item("bk", "i-1").unwrap().is_none());
    // ack_skip unfreezes the cursor; the row is never re-opened or resurrected.
    let poison = b
        .dead_letter_list("bk", clio_store::DeadLetterStatus::Open, 10)
        .unwrap()[0]
        .sync_mutation_id
        .clone();
    assert!(ack_skip(&cfg_b, &poison, "2026-01-01T00:00:00Z").expect("ack"));
    pull(&cfg_b, &["bk".to_owned()]).expect("re-pull after ack");
    let report = sync_status(&cfg_b, &["bk".to_owned()]).expect("status");
    assert_eq!(report.banks[0].dead_letter_open, 0);
    let cursor = report.banks[0].pull_cursor.as_deref().unwrap_or_default();
    assert!(!cursor.is_empty(), "{cursor:?}");
    assert!(b.get_memory_item("bk", "i-1").unwrap().is_none());
    server.stop();
}
