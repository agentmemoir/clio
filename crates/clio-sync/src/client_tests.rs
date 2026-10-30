// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync client end to end over real TCP: push/pull
//! replication between two devices, replay idempotence, bank scoping with 403
//! on unconfigured banks, client encryption with server-blind storage and
//! keyless dead-lettering, the crash-safe cursor property, DLQ-blocked pull
//! cursors, and the masked status report.
//!
//! ## Owns
//! - Behavior coverage for `client` (push / pull / status / `ack_skip`)
//!
//! ## Does not own
//! - Apply engine semantics (`apply_tests`)
//! - Transport framing details (`server_tests`)
//!
//! ## Boundary
//!
//! Tests here break when client behavior changes; they never change it.
//!
//! Keep this module focused on sync client tests.

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation};
use crate::client::{SyncClientConfig, ack_skip, pull, push, sync_status};
use crate::credential_fixtures::scoped_store;
use crate::crypto::SyncCipherKey;
use crate::protocol::{SyncMutation, decode_cursor};
use crate::server::{SyncServer, SyncServerOptions, sync_serve};
use clio_store::{
    AssocEdgeRecord, AssocKind, DekProvider, LocalDevKms, SqliteStore, Store, SyncStore,
};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

pub(crate) const TOKEN: &str = "tok-shared";

pub(crate) fn sqlite() -> Arc<SqliteStore> {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    Arc::new(SqliteStore::open_in_memory(kms).expect("sqlite open"))
}

pub(crate) fn serve(banks: &[&str]) -> (SyncServer, Arc<SqliteStore>, Arc<dyn DekProvider>) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    // The bearer secret every client config presents (`TOKEN`) is a scoped
    // record, so push/pull auth reaches a record the server loaded at start.
    let credentials = scoped_store("client-serve", TOKEN, banks);
    let server = sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: store.clone(),
        sync: store.clone(),
        kms: Arc::clone(&kms),
        banks: banks.iter().map(|b| (*b).to_owned()).collect(),
        token_file: Some(credentials.path().to_path_buf()),
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
        sync_key: None,
    })
    .expect("serve");
    (server, store, kms)
}

pub(crate) fn cfg(
    device: &str,
    store: &Arc<SqliteStore>,
    addr: &str,
    cipher: Vec<SyncCipherKey>,
) -> SyncClientConfig {
    SyncClientConfig {
        device_id: device.into(),
        store: store.clone(),
        sync: store.clone(),
        remote: Some(addr.to_owned()),
        token: Some(TOKEN.to_owned()),
        cipher,
    }
}

/// The one sync key every paired-transfer test shares: the sender seals with it
/// and the receiver opens with it, so a roundtrip in those tests cannot pass by
/// shipping plaintext. One key is derived per process, so every call hands out
/// the same one.
pub(crate) fn paired_key() -> SyncCipherKey {
    static KEY: std::sync::OnceLock<SyncCipherKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("paired-transfer-key"))
    })
    .clone()
}

/// One node's config in a paired transfer: it holds [`paired_key`], so the two
/// configs a test builds against the one relay seal and open each other's rows.
pub(crate) fn paired_cfg(device: &str, store: &Arc<SqliteStore>, addr: &str) -> SyncClientConfig {
    cfg(device, store, addr, vec![paired_key()])
}

pub(crate) fn item(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> MemoryItem {
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

/// Create one plain memory item on `store`, the fixture shape the client tests
/// share. Test-only seeding for records the relay-blindness assertions push.
pub(crate) fn create_item(store: &Arc<SqliteStore>, bank: &str, id: &str, gist: &str) {
    store
        .create_memory_item(&item(bank, id, "s1", "2026-01-01T00:00:00Z", gist), "seed")
        .expect("create item");
}

pub(crate) fn edge(
    bank: &str,
    id: &str,
    source: &str,
    target: &str,
    stamp: &str,
) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: source.into(),
        target_id: target.into(),
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.5,
        last_updated_at: stamp.into(),
    }
}

#[test]
fn t24_01_push_then_pull_replicates_item_content() {
    let (server, srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    let item = item("b1", "i-1", "s1", "2026-01-01T00:00:00Z", "memoir gist");
    a.create_memory_item(&item, "actor").expect("create item");
    // (adjacent): device A never ran a local sync_serve; push works
    // purely as a client against the remote endpoint.
    assert_eq!(a.journal_max_seq("b1").unwrap(), 0);
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    // The relay journals the sealed envelope and stays blind to the content, so
    // it can never materialize the item it relayed.
    let relayed = srv.journal_list_since("b1", 0, 10).expect("relay journal");
    assert_eq!(relayed.len(), 1, "the row reached the relay");
    assert!(
        relayed[0].client_encrypted,
        "the relay holds ciphertext only"
    );
    assert!(
        srv.get_memory_item("b1", "i-1").unwrap().is_none(),
        "a sealed row never materializes on the relay"
    );
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    let got = b
        .get_memory_item("b1", "i-1")
        .unwrap()
        .expect("replicated item");
    assert_eq!(got.gist, item.gist);
    assert_eq!(got.snapshot, item.snapshot);
    assert_eq!(got.subject_id, "s1");
    server.stop();
}

#[test]
fn t24_02_replay_push_and_pull_create_no_duplicates() {
    let (server, srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    a.create_memory_item(
        &item("b1", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    let seq = srv.journal_max_seq("b1").unwrap();
    assert_eq!(seq, 1);
    push(&cfg_a, &["b1".to_owned()]).expect("replay push");
    assert_eq!(srv.journal_max_seq("b1").unwrap(), seq);
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    pull(&cfg_b, &["b1".to_owned()]).expect("replay pull");
    let rows = b.journal_list_since("b1", 0, 200).unwrap();
    let mut ids: Vec<String> = rows.iter().map(|r| r.sync_mutation_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), rows.len());
    assert_eq!(rows.len(), 1);
    assert!(b.get_memory_item("b1", "i-1").unwrap().is_some());
    server.stop();
}

#[test]
fn t24_06_assoc_edge_syncs_and_lists_on_peer() {
    let (server, _srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    for id in ["i-1", "i-2"] {
        a.create_memory_item(&item("b1", id, "s1", "2026-01-01T00:00:00Z", id), "actor")
            .unwrap();
    }
    let e = edge("b1", "e-1", "i-1", "i-2", "2026-01-01T02:00:00Z");
    a.put_assoc_edge(&e).expect("put edge");
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    pull(&cfg_b, &["b1".to_owned()]).expect("pull");
    let edges = b.list_assoc_edges("b1", None).expect("list edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id, "e-1");
    assert_eq!(edges[0].source_id, "i-1");
    assert_eq!(edges[0].target_id, "i-2");
    server.stop();
}

#[test]
fn bank_scope_unconfigured_bank_is_403_and_b1_flows() {
    let (server, srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    a.create_memory_item(
        &item("b1", "i-b1", "s1", "2026-01-01T00:00:00Z", "b1 item"),
        "actor",
    )
    .unwrap();
    a.create_memory_item(
        &item("b2", "i-b2", "s1", "2026-01-01T00:00:00Z", "b2 item"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b1".to_owned()]).expect("push b1");
    let err = push(&cfg_a, &["b2".to_owned()]).expect_err("unconfigured bank rejected");
    assert!(
        err.message.contains("403"),
        "unexpected error: {}",
        err.message
    );
    pull(&cfg_b, &["b1".to_owned()]).expect("pull b1");
    assert!(b.get_memory_item("b1", "i-b1").unwrap().is_some());
    assert!(b.get_memory_item("b2", "i-b2").unwrap().is_none());
    assert!(srv.get_memory_item("b2", "i-b2").unwrap().is_none());
    server.stop();
}

#[test]
fn t24_07_client_encryption_server_blind_keyless_dead_letters() {
    let (server, srv, _kms) = serve(&["bc"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let c = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("t24-07-key"));
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    let cfg_c = cfg("dev-c", &c, &addr, Vec::new());
    a.create_memory_item(
        &item(
            "bc",
            "i-1",
            "s1",
            "2026-01-01T00:00:00Z",
            "t24-07 secret gist marker",
        ),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["bc".to_owned()]).expect("push");
    // The server journal holds ciphertext only; the server store stays blind.
    let rows = srv.journal_list_since("bc", 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].client_encrypted);
    let sealed = rows[0].content_sealed.as_deref().expect("sealed content");
    assert!(!sealed.contains("secret gist marker"));
    assert!(srv.get_memory_item("bc", "i-1").unwrap().is_none());
    // A peer with the same key decrypts and reads the item.
    pull(&cfg_b, &["bc".to_owned()]).expect("pull with key");
    let got = b
        .get_memory_item("bc", "i-1")
        .unwrap()
        .expect("decrypted item");
    assert_eq!(got.gist.as_deref(), Some("t24-07 secret gist marker"));
    // A keyless peer quarantines instead of silently dropping the row.
    pull(&cfg_c, &["bc".to_owned()]).expect("pull without key");
    assert_eq!(
        c.dead_letter_count("bc", clio_store::DeadLetterStatus::Open)
            .unwrap(),
        1
    );
    assert!(c.get_memory_item("bc", "i-1").unwrap().is_none());
    let poison = c
        .dead_letter_list("bc", clio_store::DeadLetterStatus::Open, 10)
        .unwrap()[0]
        .sync_mutation_id
        .clone();
    assert!(ack_skip(&cfg_c, &poison, "2026-01-01T00:00:00Z").expect("ack skip"));
    assert!(!ack_skip(&cfg_c, &poison, "2026-01-01T00:00:00Z").expect("second ack"));
    server.stop();
}

#[test]
fn t24_08_crash_between_page_and_cursor_is_idempotent() {
    let (server, srv, _kms) = serve(&["b8"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    a.create_memory_item(
        &item("b8", "i-1", "s1", "2026-01-01T00:00:00Z", "one"),
        "actor",
    )
    .unwrap();
    a.create_memory_item(
        &item("b8", "i-2", "s1", "2026-01-01T01:00:00Z", "two"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b8".to_owned()]).expect("push");
    // Crash simulation: apply the first journaled page durably, then "die"
    // before the pull cursor is persisted.
    let page = srv.journal_list_since("b8", 0, 1).unwrap();
    assert_eq!(page.len(), 1);
    let sealed = page[0].content_sealed.as_deref().expect("sealed content");
    let relayed: SyncMutation = serde_json::from_str(sealed).expect("relay envelope");
    let envelope = serde_json::to_string(&relayed).expect("envelope text");
    let m = super::decrypt_for_apply(&cfg_b, &relayed, &envelope)
        .expect("open the held row")
        .expect("the held key opens the row");
    let engine = ApplyEngine {
        store: b.as_ref(),
        sync: b.as_ref(),
        device_id: "dev-b".into(),
        kms: None,
    };
    assert_eq!(
        apply_mutation(&engine, &m, &envelope).expect("apply page"),
        ApplyOutcome::Applied
    );
    // Restart: a fresh pull re-fetches page 1 and absorbs it idempotently.
    pull(&cfg_b, &["b8".to_owned()]).expect("restart pull");
    let rows = b.journal_list_since("b8", 0, 200).unwrap();
    let mut ids: Vec<String> = rows.iter().map(|r| r.sync_mutation_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), rows.len());
    assert_eq!(rows.len(), 2);
    assert!(b.get_memory_item("b8", "i-1").unwrap().is_some());
    assert!(b.get_memory_item("b8", "i-2").unwrap().is_some());
    let report = sync_status(&cfg_b, &["b8".to_owned()]).expect("status");
    let cursor = report.banks[0].pull_cursor.as_deref().expect("pull cursor");
    assert!(decode_cursor(cursor, "b8").expect("decode") <= srv.journal_max_seq("b8").unwrap());
    server.stop();
}

#[test]
fn p_dlq_blocks_pull_cursor_until_key_or_ack_context() {
    let (server, _srv, _kms) = serve(&["b1"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    // The peer shreds the subject, so the incoming create must dead-letter on
    // the erasure check rather than on the wire refusal: both nodes hold the
    // sync key, so the row opens and apply is what rejects it.
    b.upsert_subject_key_ref("s-victim", "shredded", "shredded")
        .expect("shred marker");
    a.create_memory_item(
        &item("b1", "i-1", "s-victim", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["b1".to_owned()]).expect("push");
    pull(&cfg_b, &["b1".to_owned()]).expect("pull quarantines");
    let report = sync_status(&cfg_b, &["b1".to_owned()]).expect("status");
    assert_eq!(report.banks[0].dead_letter_open, 1);
    let cursor = report.banks[0].pull_cursor.as_deref().unwrap_or_default();
    assert!(cursor.is_empty(), "{cursor:?}");
    assert!(b.get_memory_item("b1", "i-1").unwrap().is_none());
    // Re-pull stays frozen at the poisoned id.
    pull(&cfg_b, &["b1".to_owned()]).expect("re-pull");
    let report = sync_status(&cfg_b, &["b1".to_owned()]).expect("status");
    assert_eq!(report.banks[0].dead_letter_open, 1);
    let cursor = report.banks[0].pull_cursor.as_deref().unwrap_or_default();
    assert!(cursor.is_empty(), "{cursor:?}");
    let poison = b
        .dead_letter_list("b1", clio_store::DeadLetterStatus::Open, 10)
        .unwrap()[0]
        .sync_mutation_id
        .clone();
    assert!(ack_skip(&cfg_b, &poison, "2026-01-01T00:00:00Z").expect("ack skip"));
    // After ack_skip the re-pull treats the acked id as a duplicate: the row
    // never re-opens and the cursor advances past it.
    pull(&cfg_b, &["b1".to_owned()]).expect("re-pull after ack");
    let report = sync_status(&cfg_b, &["b1".to_owned()]).expect("status");
    assert_eq!(report.banks[0].dead_letter_open, 0);
    let cursor = report.banks[0].pull_cursor.as_deref().unwrap_or_default();
    assert!(!cursor.is_empty(), "{cursor:?}");
    // The skipped content was never resurrected.
    assert!(b.get_memory_item("b1", "i-1").unwrap().is_none());
    server.stop();
}
