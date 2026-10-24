// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the regression coverage of the sealed
//! envelope binding: a clear-text execution field rewritten after the content
//! was sealed is refused before any effect, while the untouched row still
//! applies.
//!
//! ## Owns
//! - Hostile-relay retarget coverage for the destructive operations
//! - Per-field authentication coverage of the execution envelope
//! - The positive control, the replay control, and the key-scoped control
//!
//! ## Does not own
//! - How the envelope encodes to bytes (`envelope`, `envelope_tests`)
//! - The cipher primitives and their wrong-key refusals (`crypto_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the envelope binding or the refusal shape changes;
//! they never change either.
//!
//! Keep this module focused on envelope-binding regressions.

use crate::client::SyncClientConfig;
use crate::client::client_tests::{TOKEN, cfg, create_item, item, serve, sqlite};
use crate::client::{pull, sync_status};
use crate::crypto::SyncCipherKey;
use crate::http::post_json;
use crate::protocol::{
    ENTITY_KIND_ITEM, ENTITY_KIND_TASK, OP_DISCARD, OP_INVALIDATE, OP_UPSERT, PushAck, SyncContent,
    SyncMutation, SyncPayload,
};
use crate::server::SyncServer;
use clio_store::{DeadLetterStatus, Store, SyncStore};
use serde_json::json;
use std::sync::Arc;

type TestStore = Arc<clio_store::SqliteStore>;

pub(crate) const BANK: &str = "bank-a";
const OTHER_BANK: &str = "bank-b";
const STAMP: &str = "2026-01-01T00:00:00Z";
pub(crate) const VICTIM: &str = "victim-a";
const SOURCE: &str = "source-a";
/// The tail every failed open carries; the refusal names the open failure and
/// never the rewrite, so a relay cannot learn which field it mistyped.
const OPEN_FAILED: &str = "could not be opened with the matching held key";

/// One receiving node: its own relay endpoint, its own store, one held key.
pub(crate) struct Receiver {
    pub(crate) server: SyncServer,
    pub(crate) addr: String,
    pub(crate) store: TestStore,
    pub(crate) cfg: SyncClientConfig,
}

/// One held key: the tag picks a generated secret, so a test that hands the
/// same key to two nodes is the only way they can open each other's rows.
pub(crate) fn held_key(tag: &str) -> SyncCipherKey {
    SyncCipherKey::from_secret(&crate::credential_fixtures::secret(tag))
}

/// Bind a fresh relay for `banks`, a fresh store holding `key`, and the client
/// config that pulls through both.
pub(crate) fn receiver(banks: &[&str], key: &SyncCipherKey) -> Receiver {
    let (server, _srv, _kms) = serve(banks);
    let addr = server.addr().to_owned();
    let store = sqlite();
    let cfg = cfg("dev-a", &store, &addr, vec![key.clone()]);
    Receiver {
        server,
        addr,
        store,
        cfg,
    }
}

fn mutation_id(id: &str, stamp: &str, op: &str) -> String {
    clio_store::sha256_hex(
        crate::keyed_roles::published_mutation_id_salt(),
        &format!("{id}|{stamp}|{op}"),
    )
}

/// One legitimately shaped item mutation, sealed under its own execution
/// envelope: the row a well-behaved writer would have produced.
fn sealed(bank: &str, id: &str, op: &str, key: &SyncCipherKey) -> SyncMutation {
    let fixture = item(bank, id, "s1", STAMP, "source gist");
    let plain = serde_json::to_value(&fixture).expect("serialize the item");
    crate::envelope_fixtures::seal_content_for(
        SyncMutation {
            sync_mutation_id: mutation_id(id, STAMP, op),
            bank: bank.to_owned(),
            entity_kind: ENTITY_KIND_ITEM.into(),
            entity_id: id.to_owned(),
            op: op.to_owned(),
            transaction_time: STAMP.into(),
            device_id: "dev-hand".into(),
            content: SyncContent::Plain(serde_json::Value::Null),
            content_hash: clio_store::sha256_hex(
                crate::keyed_roles::published_digest_salt(),
                "anything",
            ),
            admission_receipt: None,
        },
        key,
        &plain,
    )
}

/// Push one mutation the caller shaped by hand. The payload bank is chosen
/// separately from the mutation's own bank, so a test can split the two and
/// find out which one the relay routes on.
pub(crate) fn hand_push(addr: &str, payload_bank: &str, mutation: SyncMutation) {
    let ack: PushAck = post_json(
        addr,
        "/sync/push",
        Some(TOKEN),
        &serde_json::to_value(&SyncPayload {
            device_id: mutation.device_id.clone(),
            bank: payload_bank.to_owned(),
            mutations: vec![mutation],
        })
        .expect("payload"),
    )
    .expect("hand push");
    assert!(ack.rejected.is_empty(), "the relay stores the row verbatim");
}

/// The one open dead letter `bank` holds, asserted to be the failed-open
/// refusal rather than some other reject.
fn refused(r: &Receiver, bank: &str) {
    let open = r
        .store
        .dead_letter_list(bank, DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(open.len(), 1, "exactly one row is quarantined in {bank}");
    assert_eq!(
        open[0].reason_code, "malformed",
        "a re-addressed cipher is refused as malformed"
    );
    assert!(
        open[0].detail.contains(OPEN_FAILED),
        "the refusal names the open failure: {}",
        open[0].detail
    );
}

/// The receiver pulled, refused, and left the named row untouched: the cursor
/// is frozen and the source id never landed.
fn refused_without_effect(r: &Receiver, bank: &str, applied: &str) {
    refused(r, bank);
    assert!(
        r.store
            .get_memory_item(bank, applied)
            .expect("get")
            .is_none(),
        "the refused row applied nothing"
    );
    let report = sync_status(&r.cfg, &[bank.to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_none(),
        "the cursor did not advance past a refused row"
    );
}

/// The main case, shared by the two destructive operations: a captured
/// mutation for another item is re-addressed to the live victim with a fresh
/// id, a fresh stamp, and a relay device id, and the cipher bytes travel
/// unmodified.
fn retarget_is_refused(op: &str) {
    let key = held_key("envelope-retarget-key");
    let r = receiver(&[BANK], &key);
    create_item(&r.store, BANK, VICTIM, "victim gist");
    let captured = sealed(BANK, SOURCE, op, &key);
    let mut retargeted = captured;
    retargeted.entity_id = VICTIM.into();
    retargeted.sync_mutation_id = mutation_id("synthetic-relay", STAMP, op);
    retargeted.transaction_time = "2026-01-02T00:00:00Z".into();
    retargeted.device_id = "synthetic-relay".into();
    hand_push(&r.addr, BANK, retargeted);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    refused_without_effect(&r, BANK, SOURCE);
    let identity = r
        .store
        .get_item_identity(BANK, VICTIM)
        .expect("identity read")
        .expect("the victim is still live");
    assert_eq!(identity.item_id, VICTIM);
    r.server.stop();
}

#[test]
fn hostile_relay_retarget_of_captured_discard_does_not_discard_victim() {
    retarget_is_refused(OP_DISCARD);
}

#[test]
fn hostile_relay_retarget_of_captured_invalidate_does_not_invalidate_victim() {
    retarget_is_refused(OP_INVALIDATE);
}

/// Rewrite exactly one authenticated envelope field of a delivered row.
fn tampered(field: &str, m: &mut SyncMutation) {
    match field {
        "sync_mutation_id" => m.sync_mutation_id = mutation_id("synthetic-relay", STAMP, OP_UPSERT),
        "entity_kind" => m.entity_kind = ENTITY_KIND_TASK.into(),
        "entity_id" => m.entity_id = VICTIM.into(),
        "op" => m.op = OP_DISCARD.into(),
        "transaction_time" => m.transaction_time = "2026-01-02T00:00:00Z".into(),
        "device_id" => m.device_id = "synthetic-relay".into(),
        "content_hash" => {
            m.content_hash =
                clio_store::sha256_hex(crate::keyed_roles::published_digest_salt(), "rewritten");
        }
        "admission_receipt" => m.admission_receipt = Some(json!({"admitted": true, "score": 0.9})),
        "bank" => m.bank = OTHER_BANK.into(),
        other => panic!("no rewrite defined for envelope field {other}"),
    }
}

/// Every authenticated envelope field is covered by the seal: one captured
/// cipher, one rewritten field per delivery, and a refusal each time.
#[test]
fn each_envelope_field_is_authenticated() {
    let fields = [
        "sync_mutation_id",
        "entity_kind",
        "entity_id",
        "op",
        "transaction_time",
        "device_id",
        "content_hash",
        "admission_receipt",
        "bank",
    ];
    // The captured cipher is sealed once and delivered nine times; only the
    // clear-text envelope differs. Both banks are served so the relay accepts
    // the row in either bank and the refusal is the client's, not a 403.
    let key = held_key("envelope-field-key");
    let captured = sealed(BANK, SOURCE, OP_UPSERT, &key);
    for field in fields {
        let r = receiver(&[BANK, OTHER_BANK], &key);
        create_item(&r.store, BANK, VICTIM, "victim gist");
        let mut delivered = captured.clone();
        tampered(field, &mut delivered);
        // The case must rewrite one field and nothing else, or it proves
        // nothing about that field.
        let (before, after) = (
            serde_json::to_value(&captured).expect("captured envelope"),
            serde_json::to_value(&delivered).expect("delivered envelope"),
        );
        let (before, after) = (
            before.as_object().expect("envelope object"),
            after.as_object().expect("envelope object"),
        );
        // A field the wire omits when absent appears on one side only, so the
        // comparison walks the union of both key sets, sorted.
        let mut keys: Vec<&str> = before
            .keys()
            .chain(after.keys())
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let changed: Vec<&str> = keys
            .into_iter()
            .filter(|key| after.get(*key) != before.get(*key))
            .collect();
        assert_eq!(changed, vec![field], "the case rewrites exactly {field}");
        // The row is journaled under the mutation's own bank, which is what a
        // rewritten bank field moves; pull that bank so the page serves.
        let row_bank = delivered.bank.clone();
        hand_push(&r.addr, BANK, delivered);
        pull(&r.cfg, std::slice::from_ref(&row_bank)).expect("pull");
        refused_without_effect(&r, &row_bank, SOURCE);
        let identity = r
            .store
            .get_item_identity(BANK, VICTIM)
            .expect("identity read")
            .expect("the victim is still live");
        assert_eq!(identity.item_id, VICTIM, "field {field} left it live");
        r.server.stop();
    }
}

/// The positive control: the same sealed row travels unmodified and applies,
/// so the refusals above are about the binding and not about the harness.
#[test]
fn untouched_captured_cipher_still_applies() {
    let key = held_key("envelope-control-key");
    let r = receiver(&[BANK], &key);
    hand_push(&r.addr, BANK, sealed(BANK, SOURCE, OP_UPSERT, &key));
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    let got = r
        .store
        .get_memory_item(BANK, SOURCE)
        .expect("get")
        .expect("the source item landed");
    assert_eq!(got.gist.as_deref(), Some("source gist"));
    assert_eq!(
        r.store
            .dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("count"),
        0,
        "nothing was quarantined"
    );
    let report = sync_status(&r.cfg, &[BANK.to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_some(),
        "the cursor advanced past an applied row"
    );
    r.server.stop();
}

/// Replay: the same mutation id twice produces one effect and one journal row.
#[test]
fn replay_of_the_same_mutation_id_does_not_apply_twice() {
    let key = held_key("envelope-replay-key");
    let r = receiver(&[BANK], &key);
    let captured = sealed(BANK, SOURCE, OP_UPSERT, &key);
    hand_push(&r.addr, BANK, captured.clone());
    pull(&r.cfg, &[BANK.to_owned()]).expect("first pull");
    hand_push(&r.addr, BANK, captured);
    pull(&r.cfg, &[BANK.to_owned()]).expect("second pull");
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        1,
        "the redelivery appended no second journal row"
    );
    let rows = r.store.journal_list_since(BANK, 0, 200).expect("journal");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].sync_mutation_id,
        mutation_id(SOURCE, STAMP, OP_UPSERT)
    );
    let got = r
        .store
        .get_memory_item(BANK, SOURCE)
        .expect("get")
        .expect("the item landed once");
    assert_eq!(got.gist.as_deref(), Some("source gist"));
    r.server.stop();
}

/// The key control: a cipher sealed for one envelope does not open for
/// another, and a valid delivery under the same held key still applies.
#[test]
fn cipher_sealed_for_one_mutation_does_not_open_for_another() {
    let key = held_key("envelope-rotation-key");
    let r = receiver(&[BANK, OTHER_BANK], &key);
    create_item(&r.store, BANK, VICTIM, "victim gist");
    let mut as_another = sealed(BANK, SOURCE, OP_DISCARD, &key);
    as_another.entity_id = VICTIM.into();
    hand_push(&r.addr, BANK, as_another);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    refused_without_effect(&r, BANK, SOURCE);
    assert!(
        r.store
            .get_item_identity(BANK, VICTIM)
            .expect("identity read")
            .is_some(),
        "the victim outlived the resealed row"
    );
    // Same node, same held key, a genuinely sealed row: it applies.
    hand_push(
        &r.addr,
        OTHER_BANK,
        sealed(OTHER_BANK, SOURCE, OP_UPSERT, &key),
    );
    pull(&r.cfg, &[OTHER_BANK.to_owned()]).expect("pull the valid row");
    assert!(
        r.store
            .get_memory_item(OTHER_BANK, SOURCE)
            .expect("get")
            .is_some(),
        "a valid delivery under the held key still applies"
    );
    r.server.stop();
}
