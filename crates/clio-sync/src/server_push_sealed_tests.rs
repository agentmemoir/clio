// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the push route when the serving peer holds a sync key and an
//! erasure-denial authority: the sealed row whose resolved subject a recorded
//! denial covers, the sealed row a denied authority refuses, the sealed row that
//! opens and applies, and the sealed row that opens and dead-letters.
//!
//! ## Owns
//! - Behavior coverage for the sealed-subject check and the sealed apply path of
//!   the push route
//!
//! ## Does not own
//! - The denial guards themselves (`denial_guard_sealed_tests`)
//! - The bank authorization matrix (`server_scope_tests`)
//! - The plaintext refusal and covered-bank authorization
//!   (`server_push_denial_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the push route stops resolving a sealed subject or
//! stops opening a sealed row before apply; they never change it.
//!
//! Keep this module focused on the sealed push path.

use crate::apply::apply_tests::item_value;
use crate::client::client_tests::paired_key;
use crate::credential_fixtures::{Scratch, record, secret};
use crate::denial_fixtures::FakeDenial;
use crate::envelope_fixtures::seal_content_for;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncMutation};
use crate::server::server_tests::{http, push_body, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{DeadLetterStatus, ErasureDenial, SqliteStore, Store, SyncStore};
use serde_json::{Value, json};
use std::sync::Arc;

/// The one bank every test here pushes under, and one fixed stamp.
const BANK: &str = "b-a";
const STAMP: &str = "2026-01-01T00:00:00Z";

/// A running server that holds the paired key and an in-memory authority, with
/// the credential the tests present and the two banks it may reach.
fn serve_sealed() -> (
    SyncServer,
    Arc<SqliteStore>,
    Arc<FakeDenial>,
    Scratch,
    String,
) {
    let scratch = Scratch::new("push-sealed");
    let token = secret("s");
    scratch.write(&[record("client-sealed", &token, &[BANK, "b-b"])]);
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let server = sync_serve_with_denial(
        SyncServerOptions {
            bind: "127.0.0.1:0".into(),
            store: Arc::clone(&store) as Arc<dyn Store>,
            sync: Arc::clone(&store) as Arc<dyn SyncStore>,
            kms,
            banks: vec![BANK.into(), "b-b".into()],
            token_file: Some(scratch.path().to_path_buf()),
            dev_mode: false,
            allow_public_bind: false,
            ttl_secs: None,
            deadline_secs: None,
            device_id: "srv-1".into(),
            sync_key: Some(Arc::new(paired_key())),
            policy: crate::freshness::FreshnessPolicy::default(),
        },
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, store, denial, scratch, token)
}

/// One item mutation for `bank` whose content is sealed under the peer's key, so
/// only the serving peer's sync key can read its subject.
fn sealed_item(mid: &str, bank: &str, id: &str, subject: &str, payload_bank: &str) -> SyncMutation {
    let document = item_value(payload_bank, id, subject, STAMP, "sealed");
    let mutation = SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: id.into(),
        op: OP_UPSERT.into(),
        transaction_time: STAMP.into(),
        device_id: "dev-a".into(),
        content: crate::protocol::SyncContent::Plain(document.clone()),
        content_hash: format!("h-{mid}"),
        admission_receipt: None,
    };
    seal_content_for(mutation, &paired_key(), &document)
}

/// One push of `mutations` under `bank`.
fn push(addr: &str, token: &str, bank: &str, mutations: &[SyncMutation]) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/push",
        Some(token),
        Some(&push_body(bank, mutations)),
    )
}

/// The reason codes of the open dead letters `store` holds for `bank`.
fn open_reasons(store: &SqliteStore, bank: &str) -> Vec<String> {
    store
        .dead_letter_list(bank, DeadLetterStatus::Open, 10)
        .expect("dead letters")
        .into_iter()
        .map(|row| row.reason_code)
        .collect()
}

/// A sealed row whose resolved subject a recorded denial covers is quarantined
/// as the erased subject before any write, while a clear sealed row in the same
/// push still applies.
#[test]
fn a_sealed_row_for_a_denied_subject_is_refused_while_a_clear_row_applies() {
    let (server, store, denial, _scratch, token) = serve_sealed();
    let addr = server.addr().to_owned();
    denial.deny("s-held", &[BANK]);
    let held = sealed_item("m-held", BANK, "i-held", "s-held", BANK);
    let clear = sealed_item("m-clear", BANK, "i-clear", "s-clear", BANK);
    let (status, ack) = push(&addr, &token, BANK, &[held, clear]);
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["m-clear"]), "{ack}");
    assert_eq!(ack["rejected"], json!(["m-held"]), "{ack}");
    assert_eq!(
        open_reasons(&store, BANK),
        vec!["erased_subject".to_owned()],
        "the denied subject is quarantined as such"
    );
    assert!(
        store
            .get_memory_item(BANK, "i-held")
            .expect("get")
            .is_none(),
        "the denied row writes nothing"
    );
    assert!(
        store
            .get_memory_item(BANK, "i-clear")
            .expect("get")
            .is_some(),
        "the clear row in the same push applies"
    );
    server.stop();
}

/// A sealed row whose subject is clear applies through the opened-content path:
/// the peer opens the row with its own key and the effects see the payload the
/// writer sealed.
#[test]
fn a_sealed_row_with_a_clear_subject_applies_through_the_opened_content() {
    let (server, store, _denial, _scratch, token) = serve_sealed();
    let addr = server.addr().to_owned();
    let clear = sealed_item("m-open", BANK, "i-open", "s-open", BANK);
    let (status, ack) = push(&addr, &token, BANK, std::slice::from_ref(&clear));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["m-open"]), "{ack}");
    assert_eq!(ack["rejected"], json!([]), "{ack}");
    let got = store
        .get_memory_item(BANK, "i-open")
        .expect("get")
        .expect("the sealed row applies");
    assert_eq!(got.gist.as_deref(), Some("sealed"));
    server.stop();
}

/// A sealed row that opens but whose payload the apply matrix refuses is
/// reported rejected and quarantined rather than accepted.
#[test]
fn a_sealed_row_that_is_refused_on_apply_is_reported_rejected() {
    let (server, store, _denial, _scratch, token) = serve_sealed();
    let addr = server.addr().to_owned();
    // The payload names another bank, so the apply matrix refuses the row.
    let refused = sealed_item("m-refused", BANK, "i-refused", "s-clear", "b-b");
    let (status, ack) = push(&addr, &token, BANK, std::slice::from_ref(&refused));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!([]), "{ack}");
    assert_eq!(ack["rejected"], json!(["m-refused"]), "{ack}");
    assert_eq!(
        open_reasons(&store, BANK),
        vec!["unauthorized_bank".to_owned()]
    );
    assert!(
        store
            .get_memory_item(BANK, "i-refused")
            .expect("get")
            .is_none(),
        "a refused row writes nothing"
    );
    server.stop();
}

/// A push whose authority cannot be read answers with the authority's own code
/// and applies nothing, so a client can tell it from a storage failure.
#[test]
fn an_unreadable_authority_refuses_a_sealed_push_with_its_own_code() {
    let (server, store, denial, _scratch, token) = serve_sealed();
    let addr = server.addr().to_owned();
    denial.damage();
    let m = sealed_item("m-damaged", BANK, "i-damaged", "s-clear", BANK);
    let (status, value) = push(&addr, &token, BANK, std::slice::from_ref(&m));
    assert_eq!(status, 500, "{value}");
    assert_eq!(value["code"], "config_corrupt", "{value}");
    assert!(
        store
            .get_memory_item(BANK, "i-damaged")
            .expect("get")
            .is_none(),
        "an unreadable authority applies nothing"
    );
    server.stop();
}

/// A mutation naming a bank the credential cannot reach is quarantined under the
/// authorized envelope bank and reported rejected, never written into the bank
/// it names.
#[test]
fn a_mutation_for_an_unauthorized_bank_is_refused_under_the_envelope_bank() {
    let scratch = Scratch::new("push-narrow");
    let token = secret("n");
    scratch.write(&[record("client-narrow", &token, &[BANK])]);
    let (store, kms) = sqlite();
    let server = crate::server::sync_serve(SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: Arc::clone(&store) as Arc<dyn SyncStore>,
        kms,
        banks: vec![BANK.into(), "b-b".into()],
        token_file: Some(scratch.path().to_path_buf()),
        dev_mode: false,
        allow_public_bind: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
        sync_key: None,
        policy: crate::freshness::FreshnessPolicy::default(),
    })
    .expect("serve");
    let addr = server.addr().to_owned();
    let m = sealed_item("m-other", "b-b", "i-other", "s-clear", "b-b");
    // The envelope bank is authorized; the mutation's own bank is not. A sealed
    // row is used so the plaintext refusal cannot mask the bank refusal.
    let (status, ack) = push(&addr, &token, BANK, std::slice::from_ref(&m));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["rejected"], json!(["m-other"]), "{ack}");
    assert_eq!(
        open_reasons(&store, BANK),
        vec!["unauthorized_bank".to_owned()]
    );
    assert!(
        open_reasons(&store, "b-b").is_empty(),
        "a refusal never writes into the bank it refused"
    );
    assert_eq!(store.journal_max_seq("b-b").expect("seq"), 0);
    server.stop();
}
