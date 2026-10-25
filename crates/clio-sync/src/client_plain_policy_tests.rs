// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the receive policy for plaintext content: a node
//! configured with a sync key is sealed and refuses a plaintext row before any
//! effect, while a node with no key keeps applying the same row.
//!
//! ## Owns
//! - The sealed-mode refusal of a plaintext discard, invalidate, and denial
//!   carrier, and the no-effect assertions that go with each
//! - The keyless positive control that scopes the rule to sealed mode
//!
//! ## Does not own
//! - The envelope binding of a sealed row (`client_envelope_tests`)
//! - The wording of the refusal reasons (`client_refusal_tests`)
//!
//! ## Boundary
//!
//! These tests break when the plaintext receive policy changes; they never
//! change it.
//!
//! Keep this module focused on the plaintext receive policy.

use crate::client::client_envelope_tests::{BANK, Receiver, VICTIM, hand_push, held_key, receiver};
use crate::client::client_tests::{TOKEN, cfg, create_item, serve, sqlite};
use crate::client::{pull, pull_with_denial, sync_status};
use crate::client_refusal::PLAINTEXT_REFUSED_REASON;
use crate::crypto::SyncCipherKey;
use crate::denial::{DenialCarrier, denial_mutation};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{
    ENTITY_KIND_ITEM, OP_DISCARD, OP_INVALIDATE, REASON_MALFORMED, SyncContent, SyncMutation,
};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{
    DeadLetterStatus, DekProvider, DenialVerdict, ErasureDenial, LocalDevKms, SqliteStore, Store,
    SyncStore,
};
use serde_json::json;
use std::sync::Arc;

/// A transaction time after every fixture row, so an invalidate closes an axis
/// that is genuinely open.
const PLAIN_STAMP: &str = "2026-01-02T00:00:00Z";

/// The bank a forged carrier claims to cover. It is served by the relay, so the
/// carrier is accepted, and it is never the bank the client pulls, so the
/// relay's own denial marker cannot hide the row being refused.
const COVERED_BANK: &str = "bank-covered";

/// The same receiving node with no key at all: plaintext mode, `cfg.cipher`
/// empty, which is the documented keyless deployment.
fn keyless_receiver(banks: &[&str]) -> Receiver {
    let (server, _srv, _kms) = serve(banks);
    let addr = server.addr().to_owned();
    let store = sqlite();
    let cfg = cfg("dev-a", &store, &addr, Vec::new());
    Receiver {
        server,
        addr,
        store,
        cfg,
    }
}

/// A relay that carries the deny role, so it accepts a denial operation on push
/// and journals it. A relay with no authority refuses that row outright, which
/// would leave the pull path with nothing to refuse.
fn denial_relay(banks: &[&str]) -> (SyncServer, Arc<FakeDenial>) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    let credentials = crate::credential_fixtures::scoped_store("plain-policy-relay", TOKEN, banks);
    let denial = FakeDenial::new("fp-relay-policy");
    let server = sync_serve_with_denial(
        SyncServerOptions {
            bind: "127.0.0.1:0".into(),
            store: Arc::clone(&store) as Arc<dyn Store>,
            sync: Arc::clone(&store) as Arc<dyn SyncStore>,
            kms: Arc::clone(&kms),
            banks: banks.iter().map(|bank| (*bank).to_owned()).collect(),
            token_file: Some(credentials.path().to_path_buf()),
            dev_mode: false,
            ttl_secs: None,
            deadline_secs: None,
            device_id: "srv-1".into(),
        },
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, denial)
}

/// One plaintext mutation a relay could fabricate for `id`: no seal, no
/// authenticated envelope, and empty content, which is exactly the shape a
/// keyless writer's journal row ships as.
fn plain(bank: &str, id: &str, op: &str) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: format!("plain-relay-{op}-{id}"),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: id.to_owned(),
        op: op.to_owned(),
        transaction_time: PLAIN_STAMP.into(),
        device_id: "plain-relay".into(),
        content: SyncContent::Plain(json!({})),
        content_hash: clio_store::sha256_hex(
            crate::keyed_roles::published_digest_salt(),
            "plain-relay",
        ),
        admission_receipt: None,
    }
}

/// The receiver refused the row as a sealed-mode refusal and recorded nothing:
/// the persisted pull cursor is frozen and the journal never grew.
fn refused_plain(r: &Receiver, bank: &str) {
    let open = r
        .store
        .dead_letter_list(bank, DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(open.len(), 1, "exactly one row is quarantined in {bank}");
    assert_eq!(
        open[0].reason_code, REASON_MALFORMED,
        "a plaintext row at a sealed node is refused as malformed"
    );
    assert_eq!(
        open[0].detail, PLAINTEXT_REFUSED_REASON,
        "the refusal names sealed mode as the cause"
    );
    assert_eq!(
        r.cfg.sync.cursor_get(bank, "pull").expect("cursor"),
        None,
        "the persisted pull cursor froze before the refused row"
    );
    assert_eq!(
        r.store.journal_max_seq(bank).expect("max seq"),
        0,
        "a refused row appends no journal row"
    );
    assert!(
        r.store
            .journal_list_since(bank, 0, 200)
            .expect("journal")
            .is_empty(),
        "the journal holds nothing for a refused row"
    );
}

/// The receiver applied the row: nothing was quarantined, exactly one journal
/// row landed, and the cursor advanced past it.
fn applied(r: &Receiver, bank: &str) {
    assert_eq!(
        r.store
            .dead_letter_count(bank, DeadLetterStatus::Open)
            .expect("count"),
        0,
        "nothing is quarantined without a key"
    );
    assert_eq!(
        r.store.journal_max_seq(bank).expect("max seq"),
        1,
        "the applied row was journaled once"
    );
    let report = sync_status(&r.cfg, &[bank.to_owned()]).expect("status");
    assert!(
        report.banks[0].pull_cursor.is_some(),
        "the cursor advanced past the applied row"
    );
}

/// A sealed node pulled a plaintext row for the live victim and kept the victim
/// live: the refusal is the sealed-mode one and it left no durable trace.
fn sealed_node_refuses_plain(op: &str) {
    let key = held_key("plain-policy-key");
    let r = receiver(&[BANK], &key);
    create_item(&r.store, BANK, VICTIM, "victim gist");
    hand_push(&r.addr, BANK, plain(BANK, VICTIM, op));
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    refused_plain(&r, BANK);
    let identity = r
        .store
        .get_item_identity(BANK, VICTIM)
        .expect("identity read")
        .expect("the victim is still live");
    assert_eq!(
        identity.item_id, VICTIM,
        "the refused {op} left the victim untouched"
    );
    r.server.stop();
}

#[test]
fn sealed_node_refuses_a_plaintext_discard() {
    sealed_node_refuses_plain(OP_DISCARD);
}

#[test]
fn sealed_node_refuses_a_plaintext_invalidate() {
    sealed_node_refuses_plain(OP_INVALIDATE);
}

/// A fabricated plaintext denial carrier names an arbitrary subject: the sealed
/// node refuses the row before apply, so no durable denial is ever recorded.
#[test]
fn sealed_node_refuses_a_plaintext_denial_carrier() {
    let key: SyncCipherKey = held_key("plain-policy-denial-key");
    let (server, _relay_denial) = denial_relay(&[BANK, COVERED_BANK]);
    let addr = server.addr().to_owned();
    let subject = "arbitrary-subject";
    let carrier = DenialCarrier::new("forged-request", &[COVERED_BANK]);
    hand_push(
        &addr,
        BANK,
        denial_mutation(BANK, subject, &carrier, "plain-relay", PLAIN_STAMP),
    );
    let store = sqlite();
    let cfg = cfg("dev-a", &store, &addr, vec![key]);
    let denial = FakeDenial::new("fp-plain-policy");
    pull_with_denial(&cfg, &[BANK.to_owned()], Some(denial.as_ref())).expect("pull");
    let r = Receiver {
        server,
        addr,
        store,
        cfg,
    };
    refused_plain(&r, BANK);
    assert_eq!(
        denial.records_for(subject),
        0,
        "no durable denial was recorded for the forged subject"
    );
    assert_eq!(
        denial.banks_for(subject),
        None,
        "no bank scope was recorded for the forged subject"
    );
    assert_eq!(
        denial.verdict(subject),
        DenialVerdict::Clear,
        "the authority still answers clear"
    );
    assert_eq!(
        denial.generation(),
        0,
        "the authority recorded no denial at all"
    );
    r.server.stop();
}

/// The positive control: the same row at a node with no key applies, so the
/// rule above is scoped to sealed mode and not to plaintext content itself.
#[test]
fn keyless_node_still_applies_a_plaintext_discard() {
    let r = keyless_receiver(&[BANK]);
    create_item(&r.store, BANK, VICTIM, "victim gist");
    hand_push(&r.addr, BANK, plain(BANK, VICTIM, OP_DISCARD));
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    assert!(
        r.store
            .get_item_identity(BANK, VICTIM)
            .expect("identity read")
            .is_none(),
        "the plaintext discard applied"
    );
    applied(&r, BANK);
    r.server.stop();
}

#[test]
fn keyless_node_still_applies_a_plaintext_invalidate() {
    let r = keyless_receiver(&[BANK]);
    create_item(&r.store, BANK, VICTIM, "victim gist");
    hand_push(&r.addr, BANK, plain(BANK, VICTIM, OP_INVALIDATE));
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    let got = r
        .store
        .get_memory_item(BANK, VICTIM)
        .expect("get")
        .expect("the invalidated row is still stored");
    assert_eq!(
        got.valid_until.as_deref(),
        Some(PLAIN_STAMP),
        "the plaintext invalidate closed the valid axis"
    );
    applied(&r, BANK);
    r.server.stop();
}
