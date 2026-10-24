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
//! - The positive control, the replay control, and the rotation controls
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
use crate::client::{ack_skip, pull, sync_status};
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
/// A second legitimate writer id, so a row that must still apply after a
/// refusal is not the row that was refused.
const SOURCE_AFTER: &str = "source-b";
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

/// The one open dead letter `bank` holds, asserted to be a refusal whose
/// detail names `expected`: the key it names, or the open it failed.
fn refused_naming(r: &Receiver, bank: &str, expected: &str) {
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
        open[0].detail.contains(expected),
        "the refusal names {expected}: {}",
        open[0].detail
    );
}

/// The receiver pulled and left the named row untouched: no store write, no
/// journal row, and a cursor still frozen at the prior position.
fn left_without_effect(r: &Receiver, bank: &str, applied: &str) {
    assert!(
        r.store
            .get_memory_item(bank, applied)
            .expect("get")
            .is_none(),
        "the refused row applied nothing"
    );
    // The rule is "before store, journal or cursor effects": a refused row
    // never reaches apply, so its bank's journal is still empty at seq 0.
    assert_eq!(
        r.store.journal_max_seq(bank).expect("max seq"),
        0,
        "the refusal appended no journal row in {bank}"
    );
    let journal = r.store.journal_list_since(bank, 0, 200).expect("journal");
    assert!(
        journal.is_empty(),
        "the refusal left {} journal rows",
        journal.len()
    );
    let report = sync_status(&r.cfg, &[bank.to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_none(),
        "the cursor did not advance past a refused row"
    );
}

/// The failed-open refusal, with no effect of any kind.
fn refused_without_effect(r: &Receiver, bank: &str, applied: &str) {
    refused_naming(r, bank, OPEN_FAILED);
    left_without_effect(r, bank, applied);
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

/// The rotation control, following the rotation rules the client obeys: the
/// node holds only the key it kept, so a row sealed under the key it rotated
/// away from is quarantined, and a row genuinely sealed under the held key
/// applies in that same bank once the unopenable row is discarded.
#[test]
fn rotation_control_row_sealed_under_the_rotated_away_key_is_quarantined_and_the_held_key_applies()
{
    let held = held_key("envelope-rotation-held");
    let rotated_away = held_key("envelope-rotation-away");
    let r = receiver(&[BANK], &held);
    // A peer that had already rotated wrote this row: its stamp names a key
    // this node never held, so the row cannot be selected, let alone opened.
    let stale = sealed(BANK, SOURCE, OP_UPSERT, &rotated_away);
    hand_push(&r.addr, BANK, stale.clone());
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    refused_naming(&r, BANK, &rotated_away.key_id());
    left_without_effect(&r, BANK, SOURCE);
    // The operator discards the unopenable row; the next pull of the same bank
    // delivers a row sealed under the key this node still holds, and it applies.
    assert!(ack_skip(&r.cfg, &stale.sync_mutation_id, STAMP).expect("ack skip"));
    hand_push(&r.addr, BANK, sealed(BANK, SOURCE_AFTER, OP_UPSERT, &held));
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull after the discard");
    let got = r
        .store
        .get_memory_item(BANK, SOURCE_AFTER)
        .expect("get")
        .expect("the held key's row applies in the same bank");
    assert_eq!(got.gist.as_deref(), Some("source gist"));
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        1,
        "only the row that opened reached the journal"
    );
    r.server.stop();
}

/// Key selection alone cannot retarget a seal: a destructive row sealed under
/// one held key but stamped with a second held key fails authentication under
/// the stamped key, so it is quarantined with no effect.
#[test]
fn a_key_id_naming_a_second_held_key_fails_authentication_and_applies_nothing() {
    let sealer = held_key("envelope-swap-sealer");
    let other = held_key("envelope-swap-other");
    let mut r = receiver(&[BANK], &sealer);
    // The node holds both keys, so the stamp alone decides which one is tried.
    let both = cfg(
        "dev-a",
        &r.store,
        &r.addr,
        vec![sealer.clone(), other.clone()],
    );
    r.cfg = both;
    create_item(&r.store, BANK, VICTIM, "victim gist");
    let mut swapped = sealed(BANK, SOURCE, OP_DISCARD, &sealer);
    swapped.entity_id = VICTIM.into();
    swapped.sync_mutation_id = mutation_id("synthetic-relay", STAMP, OP_DISCARD);
    if let SyncContent::Cipher(cipher) = &mut swapped.content {
        cipher.key_id = Some(other.key_id());
    }
    hand_push(&r.addr, BANK, swapped);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    // The refusal names the stamped key, so the swapped identifier is what
    // chose the key that then refused the ciphertext.
    refused_naming(&r, BANK, &other.key_id());
    left_without_effect(&r, BANK, SOURCE);
    assert!(
        r.store
            .get_item_identity(BANK, VICTIM)
            .expect("identity read")
            .is_some(),
        "the victim outlived the swapped row"
    );
    r.server.stop();
}
