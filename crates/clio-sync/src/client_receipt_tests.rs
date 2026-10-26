// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the admission receipt on the pull
//! path: that an opened sealed document restores the score onto the mutation
//! before apply, that a same-timestamp conflict is decided by that score rather
//! than by the device-id fallback, that a writer whose row carries no receipt
//! still applies, and that erasure still dead-letters under encryption.
//!
//! ## Owns
//! - Pull-path receipt restoration, the discriminating tie-break tests (item
//!   and persona), and the erasure check under encryption
//!
//! ## Does not own
//! - Packaging the receipt (`client_feed_package_receipt_tests`)
//! - Handle resolution (`client_resolve_tests`)
//! - The relay's journal-level wire assertions (`client_wire_blindness_tests`)
//! - The conflict rule itself (`apply_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the pull path stops restoring the sealed score; they
//! never change it.
//!
//! Keep this module focused on the pull-side admission receipt.

use crate::client::client_tests::{TOKEN, cfg, item, serve, sqlite};
use crate::client::{pull, push, sync_status};
use crate::crypto::SyncCipherKey;
use crate::http::post_json;
use crate::protocol::{
    ENTITY_KIND_ITEM, OP_UPSERT, PushAck, SyncContent, SyncMutation, SyncPayload,
};
use clio_store::{DeadLetterStatus, Store, SyncStore};
use clio_types::{MemoryItem, PersonaStableEntry};
use serde_json::{Value, json};

/// One bank for every test here, and one fixed stamp: no test reads the clock.
const BANK: &str = "br";
const STAMP: &str = "2026-01-01T00:00:00Z";

/// Push one hand-built mutation to the relay, so a test controls the wire shape
/// exactly as a writer of a different version would produce it.
fn hand_push(addr: &str, mutation: SyncMutation) {
    let payload = SyncPayload {
        device_id: mutation.device_id.clone(),
        bank: mutation.bank.clone(),
        mutations: vec![mutation],
    };
    let ack: PushAck = post_json(
        addr,
        "/sync/push",
        Some(TOKEN),
        &serde_json::to_value(&payload).expect("payload"),
    )
    .expect("hand push");
    assert!(ack.rejected.is_empty(), "the relay stores it verbatim");
}

/// The sealed item mutation an old writer sends: the record is sealed alone
/// (its document carries no receipt) and the wire receipt is absent.
fn sealed_without_receipt(
    cfg: &crate::client::SyncClientConfig,
    bank: &str,
    record: &MemoryItem,
    device: &str,
    mid: &str,
) -> SyncMutation {
    let key = cfg.cipher.first().expect("a held key");
    let document = serde_json::to_value(record).expect("record");
    let mutation = SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: record.id.clone(),
        op: OP_UPSERT.into(),
        transaction_time: record.updated_at.clone(),
        device_id: device.into(),
        content: SyncContent::Plain(Value::Null),
        content_hash: clio_store::sha256_hex(
            crate::keyed_roles::published_digest_salt(),
            &String::from_utf8(serde_json::to_vec(&document).expect("bytes")).unwrap_or_default(),
        ),
        admission_receipt: None,
    };
    crate::envelope_fixtures::seal_content_for(mutation, key, &document)
}

/// The wire mutation of one sealed item record carrying `receipt` in its
/// document, the shape a current writer produces.
fn sealed_with_receipt(
    key: &SyncCipherKey,
    bank: &str,
    record: &MemoryItem,
    device: &str,
    receipt: Value,
) -> SyncMutation {
    let mut document = serde_json::to_value(record).expect("record");
    document[crate::client_feed::package::ADMISSION_RECEIPT_KEY] = receipt;
    let mutation = SyncMutation {
        sync_mutation_id: format!("m-{}", record.id),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: record.id.clone(),
        op: OP_UPSERT.into(),
        transaction_time: record.updated_at.clone(),
        device_id: device.into(),
        content: SyncContent::Plain(Value::Null),
        content_hash: "unused-by-the-restoration".into(),
        admission_receipt: None,
    };
    crate::envelope_fixtures::seal_content_for(mutation, key, &document)
}

/// The restoration runs when the content opens, before `apply_mutation` sees
/// the row.
#[test]
fn an_opened_document_restores_the_receipt_before_apply() {
    let store = sqlite();
    let key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-restore-key"));
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![key.clone()]);
    let record = item(BANK, "i-1", "s1", STAMP, "restored");
    let mutation = sealed_with_receipt(
        &key,
        BANK,
        &record,
        "dev-a",
        json!({ "admitted": true, "score": 0.9 }),
    );
    let envelope = serde_json::to_string(&mutation).expect("envelope");
    let opened = super::decrypt_for_apply(&cfg, &mutation, &envelope)
        .expect("decrypt")
        .expect("an item row resolves");
    assert_eq!(
        opened.admission_receipt,
        Some(json!({ "admitted": true, "score": 0.9 })),
        "the score is back on the mutation before apply_mutation runs"
    );
}

/// A document with no score, or a malformed one, leaves the documented
/// absent-score fallback and is not an error.
#[test]
fn an_absent_or_malformed_receipt_degrades_to_the_documented_fallback() {
    let store = sqlite();
    let key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-fallback-key"));
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![key.clone()]);
    let record = item(BANK, "i-1", "s1", STAMP, "fallback");
    let cases: [(&str, Value); 2] = [
        ("no receipt key", Value::Null),
        (
            "malformed score",
            json!({ "admitted": true, "score": "high" }),
        ),
    ];
    for (label, receipt) in cases {
        let mut document = serde_json::to_value(&record).expect("record");
        if !receipt.is_null() {
            document[crate::client_feed::package::ADMISSION_RECEIPT_KEY] = receipt;
        }
        let mutation = SyncMutation {
            sync_mutation_id: format!("m-{label}"),
            bank: BANK.into(),
            entity_kind: ENTITY_KIND_ITEM.into(),
            entity_id: record.id.clone(),
            op: OP_UPSERT.into(),
            transaction_time: STAMP.into(),
            device_id: "dev-a".into(),
            content: SyncContent::Plain(Value::Null),
            content_hash: "unused-by-the-restoration".into(),
            admission_receipt: None,
        };
        let mutation = crate::envelope_fixtures::seal_content_for(mutation, &key, &document);
        let envelope = serde_json::to_string(&mutation).expect("envelope");
        let opened = super::decrypt_for_apply(&cfg, &mutation, &envelope)
            .expect("decrypt")
            .expect("the row resolves");
        assert!(
            opened.admission_receipt.is_none(),
            "{label}: the receipt stays absent, so lww_compare applies 0.0"
        );
    }
}

/// An encrypted same-timestamp conflict is decided by the restored score. The
/// device ids are chosen so a score-less tie would pick the local side, which
/// is what makes the test discriminating.
#[test]
fn an_encrypted_same_timestamp_conflict_resolves_by_the_restored_score() {
    let (server, srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-tie-key"));
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-z", &b, &addr, vec![key]);
    let mut incoming = item(BANK, "i-1", "s1", STAMP, "incoming");
    incoming.admission_score = Some(0.9);
    a.create_memory_item(&incoming, "seed").expect("incoming");
    let mut local = item(BANK, "i-1", "s1", STAMP, "local");
    local.admission_score = Some(0.0);
    b.create_memory_item(&local, "seed").expect("local");
    push(&cfg_a, &[BANK.to_owned()]).expect("push");
    let relayed = srv.journal_list_since(BANK, 0, 10).expect("relay journal");
    assert!(
        relayed.iter().all(|row| row.admission_receipt.is_none()),
        "the relay stores no receipt for any row"
    );
    pull(&cfg_b, &[BANK.to_owned()]).expect("pull");
    let got = b.get_memory_item(BANK, "i-1").expect("get").expect("row");
    assert_eq!(
        got.gist.as_deref(),
        Some("incoming"),
        "the higher sealed score wins the tie; the local device id 'dev-z' wins a score-less tie"
    );
    assert_eq!(
        b.conflict_count(BANK).expect("conflict count"),
        0,
        "the incoming row applied rather than losing"
    );
    server.stop();
}

/// The same conflict with the document receipt removed falls back to the
/// device id, so the scored test fails if the score is not restored. This is
/// the pair that proves the score is what decided the tie.
#[test]
fn the_same_conflict_without_the_score_falls_back_to_the_device_id() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let b = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-tie-key"));
    let cfg_b = cfg("dev-z", &b, &addr, vec![key]);
    let mut local = item(BANK, "i-1", "s1", STAMP, "local");
    local.admission_score = Some(0.0);
    b.create_memory_item(&local, "seed").expect("local");
    let incoming = item(BANK, "i-1", "s1", STAMP, "incoming");
    hand_push(
        &addr,
        sealed_without_receipt(&cfg_b, BANK, &incoming, "dev-a", "m-no-receipt"),
    );
    pull(&cfg_b, &[BANK.to_owned()]).expect("pull");
    let got = b.get_memory_item(BANK, "i-1").expect("get").expect("row");
    assert_eq!(
        got.gist.as_deref(),
        Some("local"),
        "without the score the device-id fallback keeps the local row"
    );
    assert_eq!(
        b.conflict_count(BANK).expect("conflicts"),
        1,
        "the incoming row is the journaled loser"
    );
    let report = sync_status(&cfg_b, &[BANK.to_owned()]).expect("status");
    let sample = &report.banks[0].conflict_sample;
    assert!(
        sample.iter().any(|entry| entry.starts_with("i-1")),
        "the conflict sample names the record: {sample:?}"
    );
    assert!(
        !sample.iter().any(|entry| entry.contains("score")
            || entry.contains("0.9")
            || entry.contains("admitted")),
        "the sample carries no score: {sample:?}"
    );
    server.stop();
}

/// A row sealed without a document receipt, the shape an old writer produces,
/// still applies on a fresh peer.
#[test]
fn a_row_sealed_without_a_receipt_still_applies_on_a_fresh_peer() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let b = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret(
        "receipt-old-writer-key",
    ));
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    let incoming = item(BANK, "i-1", "s1", STAMP, "from an old writer");
    hand_push(
        &addr,
        sealed_without_receipt(&cfg_b, BANK, &incoming, "dev-a", "m-old-writer"),
    );
    pull(&cfg_b, &[BANK.to_owned()]).expect("pull");
    let got = b
        .get_memory_item(BANK, "i-1")
        .expect("get")
        .expect("the row applies");
    assert_eq!(got.gist.as_deref(), Some("from an old writer"));
    assert_eq!(
        b.dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("dead letters"),
        0,
        "no failure path is taken"
    );
    server.stop();
}

/// A shredded subject still dead-letters with reason `erased_subject` when the
/// cipher is on.
#[test]
fn an_erased_subject_still_dead_letters_with_the_cipher_on() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-erasure-key"));
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    b.upsert_subject_key_ref("s-victim", "shredded", "shredded")
        .expect("shred marker");
    a.create_memory_item(&item(BANK, "i-1", "s-victim", STAMP, "doomed"), "actor")
        .expect("item");
    push(&cfg_a, &[BANK.to_owned()]).expect("push");
    pull(&cfg_b, &[BANK.to_owned()]).expect("pull quarantines");
    let letters = b
        .dead_letter_list(BANK, DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(letters.len(), 1, "the erased subject is refused once");
    assert_eq!(letters[0].reason_code, "erased_subject");
    assert!(
        b.get_memory_item(BANK, "i-1").expect("get").is_none(),
        "nothing was applied"
    );
    server.stop();
}

/// A sealed persona stable entry restores its score through the handle branch:
/// the wire identifier is the opaque handle, resolution and restoration run in
/// that order before apply, and the restored score decides a same-version
/// persona tie the way the item tests prove it for items. The device ids are
/// chosen so a score-less tie would keep the local row, which makes the test
/// discriminating: a restoration gated on handle resolution fails here.
#[test]
fn a_sealed_persona_tie_is_decided_by_the_restored_score() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key =
        SyncCipherKey::from_secret(&crate::credential_fixtures::secret("receipt-persona-key"));
    let packager = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-z", &b, &addr, vec![key]);
    let local = PersonaStableEntry {
        bank_id: BANK.into(),
        key: "communication_style".into(),
        value: "local value".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: None,
        admission_score: 0.0,
        as_of: STAMP.into(),
        valid_until: None,
    };
    b.persona_stable_put(&local, "seed")
        .expect("seed the local persona row");
    let mut incoming = local;
    incoming.value = "incoming value".into();
    incoming.admission_score = 0.9;
    let mutation =
        crate::client_feed::package::persona_stable_mutation(&packager, BANK, STAMP, &incoming)
            .expect("package the persona mutation");
    assert!(
        mutation.admission_receipt.is_none(),
        "the wire carries no receipt with a cipher configured"
    );
    hand_push(&addr, mutation);
    pull(&cfg_b, &[BANK.to_owned()]).expect("pull");
    let got = b
        .persona_stable_current(BANK, "communication_style")
        .expect("read the persona row")
        .expect("a persona row survives the pull");
    assert_eq!(
        got.value, "incoming value",
        "the higher sealed score wins the persona tie; the local device id 'dev-z' wins a \
         score-less tie"
    );
    assert_eq!(
        b.conflict_count(BANK).expect("conflict count"),
        0,
        "the incoming persona row applied rather than losing"
    );
    server.stop();
}
