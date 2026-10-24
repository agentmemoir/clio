// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests key rotation on the sync client: sealing with the newest
//! key, opening by the stamped identifier in either list order, the refusal of
//! identifier-less rows and rows their stamped key cannot open, with frozen
//! cursors, and the key identifier's trip through the journal round-trip.
//!
//! ## Owns
//! - Behavior coverage for multi-key sealing, opening, and refusal paths
//!
//! ## Does not own
//! - The cipher primitives themselves (`crypto_nonce_tests`, `crypto_tests`)
//! - Status reporting (`client_status_tests`)
//! - Apply matrix semantics (`apply_tests`, `apply_matrix_tests`)
//!
//! ## Boundary
//!
//! Tests here break when rotation behavior changes; they never change it.
//! Every test runs against a real sync server over TCP, on the same harness
//! `client_tests` stands up.
//!
//! Keep this module focused on the rotation rules.

use crate::client::client_tests::{TOKEN, cfg, item, serve, sqlite};
use crate::client::{ack_skip, pull, push, sync_status};
use crate::crypto::{SyncCipher, SyncCipherKey};
use crate::http::post_json;
use crate::protocol::{
    ENTITY_KIND_ITEM, OP_UPSERT, PushAck, SyncContent, SyncMutation, SyncPayload,
};
use clio_store::{DeadLetterStatus, Store as _, SyncStore as _, sha256_hex};
use std::sync::Arc;

type Store = Arc<clio_store::SqliteStore>;

/// A push payload carrying one mutation the caller shaped by hand, so a test
/// controls the wire shape exactly as any writer would have produced it.
fn hand_pushed(
    addr: &str,
    key: &SyncCipherKey,
    bank: &str,
    fixture: &clio_types::MemoryItem,
    adjust: impl FnOnce(&mut SyncCipher),
) {
    let plain = serde_json::to_value(fixture).expect("serialize the item fixture");
    let mutation = SyncMutation {
        sync_mutation_id: sha256_hex(
            crate::keyed_roles::published_mutation_id_salt(),
            &format!("{}|{}|{OP_UPSERT}", fixture.id, fixture.tx_from),
        ),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: fixture.id.clone(),
        op: OP_UPSERT.into(),
        transaction_time: fixture.tx_from.clone(),
        device_id: "dev-hand".into(),
        content: SyncContent::Plain(serde_json::Value::Null),
        content_hash: sha256_hex(
            crate::keyed_roles::published_digest_salt(),
            &serde_json::to_string(&plain).expect("plain text"),
        ),
        admission_receipt: None,
    };
    let mut mutation = crate::envelope_fixtures::seal_content_for(mutation, key, &plain);
    if let SyncContent::Cipher(cipher) = &mut mutation.content {
        adjust(cipher);
    }
    let payload = SyncPayload {
        device_id: "dev-hand".into(),
        bank: bank.to_owned(),
        mutations: vec![mutation],
    };
    let ack: PushAck = post_json(
        addr,
        "/sync/push",
        Some(TOKEN),
        &serde_json::to_value(&payload).expect("serialize payload"),
    )
    .expect("hand push");
    assert!(ack.rejected.is_empty(), "the hand-pushed row is accepted");
}

#[test]
fn seal_stamps_the_newest_key_and_both_list_orders_open() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a: Store = sqlite();
    let b: Store = sqlite();
    let c: Store = sqlite();
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    let cfg_a = cfg("dev-a", &a, &addr, vec![k2.clone()]);
    a.create_memory_item(
        &item("br", "i-1", "s1", "2026-01-01T00:00:00Z", "rotation secret"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["br".to_owned()]).expect("push");

    // The newest key sealed, and its identifier reached the journal whole.
    let rows = srv.journal_list_since("br", 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    let sealed = rows[0].content_sealed.as_deref().expect("sealed content");
    assert!(
        sealed.contains(&k2.key_id()),
        "the newest key's id is on the journal row: {sealed}"
    );
    assert!(
        !sealed.contains(&k1.key_id()),
        "an unused key's id never appears: {sealed}"
    );

    // Either list order opens: the reader matches the stamped identifier, not
    // the list position.
    let cfg_b = cfg("dev-b", &b, &addr, vec![k2.clone(), k1.clone()]);
    pull(&cfg_b, &["br".to_owned()]).expect("pull newest-first");
    assert!(b.get_memory_item("br", "i-1").unwrap().is_some());
    let cfg_c = cfg("dev-c", &c, &addr, vec![k1, k2]);
    pull(&cfg_c, &["br".to_owned()]).expect("pull oldest-first");
    assert!(c.get_memory_item("br", "i-1").unwrap().is_some());
    server.stop();
}

#[test]
fn content_sealed_under_a_superseded_key_stays_readable() {
    let (server, _srv, _kms) = serve(&["bs"]);
    let addr = server.addr().to_owned();
    let a: Store = sqlite();
    let b: Store = sqlite();
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    // The writer sealed under key one before the rotation happened.
    let cfg_a = cfg("dev-a", &a, &addr, vec![k1.clone()]);
    a.create_memory_item(
        &item("bs", "i-1", "s1", "2026-01-01T00:00:00Z", "old-key secret"),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["bs".to_owned()]).expect("push");

    // The rotated holder keeps key one and still reads everything written
    // under it; new content would be sealed with key two.
    let cfg_b = cfg("dev-b", &b, &addr, vec![k2, k1]);
    pull(&cfg_b, &["bs".to_owned()]).expect("pull with the retained key");
    let got = b
        .get_memory_item("bs", "i-1")
        .unwrap()
        .expect("the old key's content is readable");
    assert_eq!(got.gist.as_deref(), Some("old-key secret"));
    server.stop();
}

#[test]
fn a_key_id_less_row_is_refused_and_the_cursor_freezes() {
    let (server, srv, _kms) = serve(&["bt"]);
    let addr = server.addr().to_owned();
    let b: Store = sqlite();
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    // A row written before identifiers existed: valid ciphertext under key
    // one, no identifier on the wire.
    let fixture = item(
        "bt",
        "i-old",
        "s1",
        "2026-01-01T00:00:00Z",
        "identifier-less",
    );
    hand_pushed(&addr, &k1, "bt", &fixture, |cipher| cipher.key_id = None);
    let rows = srv.journal_list_since("bt", 0, 10).unwrap();
    assert!(
        !rows[0]
            .content_sealed
            .as_deref()
            .unwrap_or_default()
            .contains("key_id"),
        "the fixture carries no identifier"
    );

    // The pull completes, but the row is dead-lettered instead of opened by
    // trying every held key: nothing is applied and the cursor stays frozen.
    let cfg_b = cfg("dev-b", &b, &addr, vec![k2, k1]);
    pull(&cfg_b, &["bt".to_owned()]).expect("pull");
    assert!(b.get_memory_item("bt", "i-old").unwrap().is_none());
    let report = sync_status(&cfg_b, &["bt".to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_none(),
        "the cursor did not advance past a refused row"
    );
    let letters = b
        .dead_letter_list("bt", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(letters.len(), 1);
    let detail = &letters[0].detail;
    assert!(
        detail.contains("no key identifier"),
        "the refusal names the missing identifier: {detail}"
    );
    assert!(
        detail.contains("sync_ack_skip") && detail.contains("with confirm"),
        "the refusal names the deliberate discard: {detail}"
    );

    // The deliberate recovery path still works: ack-skip lets the cursor
    // advance past the refused row on the next pull.
    let poison = letters[0].sync_mutation_id.clone();
    assert!(ack_skip(&cfg_b, &poison, "2026-01-01T00:00:00Z").expect("ack skip"));
    pull(&cfg_b, &["bt".to_owned()]).expect("pull after ack");
    let report = sync_status(&cfg_b, &["bt".to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_some(),
        "the cursor advanced past the acked row"
    );
    server.stop();
}

#[test]
fn a_row_sealed_under_an_absent_key_is_refused_and_the_cursor_freezes() {
    let (server, _srv, _kms) = serve(&["bu"]);
    let addr = server.addr().to_owned();
    let a: Store = sqlite();
    let b: Store = sqlite();
    let kx = SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "a-key-this-node-never-held",
    ));
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    let cfg_a = cfg("dev-a", &a, &addr, vec![kx.clone()]);
    a.create_memory_item(
        &item(
            "bu",
            "i-1",
            "s1",
            "2026-01-01T00:00:00Z",
            "missing-key secret",
        ),
        "actor",
    )
    .unwrap();
    push(&cfg_a, &["bu".to_owned()]).expect("push");

    let cfg_b = cfg("dev-b", &b, &addr, vec![k2.clone(), k1.clone()]);
    pull(&cfg_b, &["bu".to_owned()]).expect("the pull itself completes");
    // The row never reached the store, the cursor froze at the last durable
    // position, and the dead letter names the missing key and the recovery.
    assert!(b.get_memory_item("bu", "i-1").unwrap().is_none());
    let report = sync_status(&cfg_b, &["bu".to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_none(),
        "the cursor did not advance past a refused row"
    );
    let letters = b
        .dead_letter_list("bu", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(letters.len(), 1);
    let detail = &letters[0].detail;
    assert!(
        detail.contains(&kx.key_id()),
        "the refusal names the missing key: {detail}"
    );
    assert!(
        detail.contains(&k2.key_id()) && detail.contains(&k1.key_id()),
        "the refusal names the keys held: {detail}"
    );
    assert!(
        detail.contains("Add the missing key on this node and pull again"),
        "the refusal names the recovery: {detail}"
    );
    assert!(
        detail.contains("sync_ack_skip") && detail.contains("with confirm"),
        "the refusal names the deliberate discard: {detail}"
    );
    // No key bytes and no content in the refusal: the ids are derived, the
    // row's plaintext never appears, and neither does any sealed blob.
    assert!(
        !detail.contains("secret"),
        "no plaintext in the refusal: {detail}"
    );
    assert!(
        !detail.contains("nonce"),
        "no ciphertext material in the refusal: {detail}"
    );

    // The deliberate recovery path still works: ack-skip lets the cursor
    // advance past the refused row on the next pull.
    let poison = letters[0].sync_mutation_id.clone();
    assert!(ack_skip(&cfg_b, &poison, "2026-01-01T00:00:00Z").expect("ack skip"));
    pull(&cfg_b, &["bu".to_owned()]).expect("pull after ack");
    let report = sync_status(&cfg_b, &["bu".to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_some(),
        "the cursor advanced past the acked row"
    );
    server.stop();
}

#[test]
fn a_row_sealed_by_a_held_key_that_no_longer_opens_is_refused_and_the_cursor_freezes() {
    let (server, _srv, _kms) = serve(&["bw"]);
    let addr = server.addr().to_owned();
    let b: Store = sqlite();
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    let fixture = item("bw", "i-bad", "s1", "2026-01-01T00:00:00Z", "tampered");
    // Stamped with an identifier the reader holds, but the ciphertext bytes
    // are corrupted: the identifier matches, the open does not.
    hand_pushed(&addr, &k1, "bw", &fixture, |cipher| {
        let mut chars: Vec<char> = cipher.ciphertext_b64.chars().collect();
        chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
        cipher.ciphertext_b64 = chars.into_iter().collect::<String>();
    });
    let stamped = k1.key_id();

    // The pull completes, the row is dead-lettered under its stamped
    // identifier, nothing is applied, and the cursor stays frozen.
    let cfg_b = cfg("dev-b", &b, &addr, vec![k2, k1]);
    pull(&cfg_b, &["bw".to_owned()]).expect("pull");
    assert!(b.get_memory_item("bw", "i-bad").unwrap().is_none());
    let report = sync_status(&cfg_b, &["bw".to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_none(),
        "the cursor did not advance past a refused row"
    );
    let letters = b
        .dead_letter_list("bw", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(letters.len(), 1);
    let detail = &letters[0].detail;
    assert!(
        detail.contains(&stamped),
        "the refusal names the stamped identifier: {detail}"
    );
    assert!(
        detail.contains("matching held key"),
        "the refusal names the open failure: {detail}"
    );
    assert!(
        detail.contains("sync_ack_skip") && detail.contains("with confirm"),
        "the refusal names the deliberate discard: {detail}"
    );
    server.stop();
}

#[test]
fn a_wider_key_list_opens_every_writer() {
    let (server, _srv, _kms) = serve(&["bv"]);
    let addr = server.addr().to_owned();
    let a: Store = sqlite();
    let b: Store = sqlite();
    let k1 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-one"));
    let k2 = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("rotation-key-two"));
    // Two writers on two generations; the reader holds both and reads both.
    let cfg_a1 = cfg("dev-a1", &a, &addr, vec![k1.clone()]);
    a.create_memory_item(
        &item(
            "bv",
            "i-1",
            "s1",
            "2026-01-01T00:00:00Z",
            "first generation",
        ),
        "actor",
    )
    .unwrap();
    push(&cfg_a1, &["bv".to_owned()]).expect("push under key one");
    let cfg_a2 = cfg("dev-a2", &a, &addr, vec![k2.clone()]);
    a.create_memory_item(
        &item(
            "bv",
            "i-2",
            "s2",
            "2026-01-02T00:00:00Z",
            "second generation",
        ),
        "actor",
    )
    .unwrap();
    push(&cfg_a2, &["bv".to_owned()]).expect("push under key two");

    let cfg_b = cfg("dev-b", &b, &addr, vec![k2, k1]);
    pull(&cfg_b, &["bv".to_owned()]).expect("pull both generations");
    assert!(b.get_memory_item("bv", "i-1").unwrap().is_some());
    assert!(b.get_memory_item("bv", "i-2").unwrap().is_some());
    server.stop();
}
