// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the push route's sealed-subject guard and its request
//! gates: a sealed row for a denied subject is quarantined before any write, a
//! sealed row this peer opens applies while the journal keeps the sealed form,
//! a sealed denial carrier from a bank token is refused before it records
//! anything, an envelope bank the token cannot reach answers forbidden, and a
//! body this route cannot decode is refused with the argument code.
//!
//! ## Owns
//! - Behavior coverage for `check_sealed_subject` on a serving peer that holds
//!   the sync key
//! - Behavior coverage for the sealed branch of `apply_and_record`
//! - Behavior coverage for a sealed denial carrier's actor gate
//! - Behavior coverage for the envelope-bank refusal and `decode_push` failures
//!
//! ## Does not own
//! - The plaintext carrier's erasure-actor and covered-bank gates
//!   (`server_push_denial_tests`)
//! - The stale-envelope refusal (`server_push_stale_tests`)
//!
//! ## Boundary
//!
//! These tests break when the sealed-subject guard or the request gates change;
//! they never change either.
//!
//! Keep this module focused on the sealed push and the request gates.

use crate::apply_fixtures::{env, item_value};
use crate::client::client_tests::paired_key;
use crate::credential_fixtures::{Scratch, record_as, secret};
use crate::crypto::SyncCipherKey;
use crate::denial::{DenialCarrier, denial_mutation, denied_bank_marker};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{
    ENTITY_KIND_ITEM, OP_UPSERT, REASON_ERASED_SUBJECT, REASON_UNAUTHORIZED_ACTOR, SyncMutation,
};
use crate::server::server_tests::{http, push_body, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{DeadLetterStatus, ErasureDenial, SqliteStore, Store, SyncStore};
use clio_types::{Actor, ErrorCode};
use serde_json::{Value, json};
use std::sync::Arc;

/// A running server that holds the sync key and an authority, with one
/// harness credential scoped to `b-a` alone and one agent credential with the
/// same scope.
struct Sealed {
    server: SyncServer,
    store: Arc<SqliteStore>,
    denial: Arc<FakeDenial>,
    _scratch: Scratch,
    token: String,
    agent: String,
}

/// Build the serving peer: the same shared key the fixtures seal under, an
/// in-memory authority, and two tokens that reach `b-a` alone.
fn serve_sealed() -> Sealed {
    let scratch = Scratch::new("push-sealed");
    let token = secret("t");
    let agent = secret("g");
    scratch.write(&[
        record_as("client-sealed", &token, Actor::Harness, &["b-a"]),
        record_as("client-agent", &agent, Actor::Agent, &["b-a"]),
    ]);
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let server = sync_serve_with_denial(
        SyncServerOptions {
            bind: "127.0.0.1:0".into(),
            store: Arc::clone(&store) as Arc<dyn Store>,
            sync: Arc::clone(&store) as Arc<dyn SyncStore>,
            kms,
            banks: vec!["b-a".into(), "b-b".into()],
            token_file: Some(scratch.path().to_path_buf()),
            dev_mode: false,
            allow_public_bind: false,
            ttl_secs: None,
            deadline_secs: None,
            max_envelope_skew_secs: None,
            device_id: "srv-1".into(),
            sync_key: Some(Arc::new(paired_key())),
        },
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    Sealed {
        server,
        store,
        denial,
        _scratch: scratch,
        token,
        agent,
    }
}

/// One item mutation sealed under the peer's key in its own execution
/// envelope, which is the only shape the push route accepts.
fn sealed(mid: &str, id: &str, subject: &str, stamp: &str, key: &SyncCipherKey) -> SyncMutation {
    let content = item_value("b-a", id, subject, stamp, "sealed gist");
    let mutation = env(
        mid,
        "b-a",
        ENTITY_KIND_ITEM,
        id,
        OP_UPSERT,
        stamp,
        "dev-a",
        content.clone(),
    );
    crate::envelope_fixtures::seal_content_for(mutation, key, &content)
}

/// The one sealed denial carrier a push sends for `subject`, covering `banks`.
fn sealed_denial(
    request_ref: &str,
    subject: &str,
    banks: &[&str],
    key: &SyncCipherKey,
) -> SyncMutation {
    let op = denial_mutation(
        "b-a",
        subject,
        &DenialCarrier::new(request_ref, banks),
        "dev-a",
        &crate::clock::now_utc(),
    );
    let content = op.content_value().clone();
    crate::envelope_fixtures::seal_content_for(op, key, &content)
}

/// One push of `mutations` under `bank` with the module's token.
fn push(addr: &str, token: &str, bank: &str, mutations: &[SyncMutation]) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/push",
        Some(token),
        Some(&push_body(bank, mutations)),
    )
}

/// A sealed row resolves its subject under this peer's key before any write,
/// so a recorded denial quarantines it as the erased subject and the
/// acknowledgement rejects it.
#[test]
fn a_sealed_row_for_a_denied_subject_is_quarantined_before_apply() {
    let s = serve_sealed();
    let addr = s.server.addr().to_owned();
    s.denial.deny("s-held", &["b-a"]);
    let held = sealed(
        "m-held",
        "i-held",
        "s-held",
        &crate::clock::now_utc(),
        &paired_key(),
    );
    let (status, ack) = push(&addr, &s.token, "b-a", std::slice::from_ref(&held));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(
        ack["accepted"],
        json!([]),
        "a denied subject is not accepted: {ack}"
    );
    assert_eq!(
        ack["rejected"],
        json!(["m-held"]),
        "the sender is told the row did not land: {ack}"
    );
    let rows = s
        .store
        .dead_letter_list("b-a", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(rows.len(), 1, "the refusal is inspectable");
    assert_eq!(
        rows[0].reason_code, REASON_ERASED_SUBJECT,
        "the row is quarantined as the erased subject"
    );
    assert!(
        s.store.get_memory_item("b-a", "i-held").unwrap().is_none(),
        "a denied row writes no content"
    );
    assert!(
        s.store.journal_get("b-a", "m-held").unwrap().is_none(),
        "a refused row is never journaled"
    );
    s.server.stop();
}

/// A sealed row for a clear subject is opened under this peer's key, applies,
/// and the journal keeps the sealed form so every reader downstream still
/// receives what the writer sealed.
#[test]
fn a_sealed_row_this_peer_opens_applies_and_journals_sealed() {
    let s = serve_sealed();
    let addr = s.server.addr().to_owned();
    let clear = sealed(
        "m-clear",
        "i-clear",
        "s-clear",
        &crate::clock::now_utc(),
        &paired_key(),
    );
    let (status, ack) = push(&addr, &s.token, "b-a", std::slice::from_ref(&clear));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["m-clear"]), "{ack}");
    assert_eq!(ack["rejected"], json!([]), "{ack}");
    assert!(
        s.store.get_memory_item("b-a", "i-clear").unwrap().is_some(),
        "the opened row applies"
    );
    let row = s
        .store
        .journal_get("b-a", "m-clear")
        .unwrap()
        .expect("the applied row is journaled");
    assert!(
        row.client_encrypted,
        "the journal relays the sealed row, never the opened plaintext"
    );
    s.server.stop();
}

/// A sealed denial carrier answers to the same actor gate as a plaintext one:
/// a bank token's sealed carrier is refused and quarantined before the opened
/// row can record the subject denial or any bank marker.
#[test]
fn a_sealed_denial_carrier_from_a_bank_token_is_refused() {
    let s = serve_sealed();
    let addr = s.server.addr().to_owned();
    let op = sealed_denial("req-sealed-agent", "s-held", &["b-a"], &paired_key());
    let (status, ack) = push(&addr, &s.agent, "b-a", std::slice::from_ref(&op));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(
        ack["accepted"],
        json!([]),
        "a sealed carrier from a bank token is not accepted: {ack}"
    );
    assert_eq!(
        ack["rejected"],
        json!(["denial:req-sealed-agent"]),
        "the sender is told the carrier did not land: {ack}"
    );
    assert!(
        ack.get("denial_acks").is_none(),
        "no proof rides a refusal: {ack}"
    );
    assert_eq!(
        s.denial.records_for("s-held"),
        0,
        "no subject denial is recorded"
    );
    assert_eq!(
        s.denial.records_for(&denied_bank_marker("b-a")),
        0,
        "the token cannot record a marker for its own bank"
    );
    assert!(
        s.store
            .journal_get("b-a", &op.sync_mutation_id)
            .unwrap()
            .is_none(),
        "a refused carrier is never journaled"
    );
    let rows = s
        .store
        .dead_letter_list("b-a", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(rows.len(), 1, "the refusal is inspectable");
    assert_eq!(
        rows[0].reason_code, REASON_UNAUTHORIZED_ACTOR,
        "the refusal names the actor gate"
    );
    s.server.stop();
}

/// An envelope bank the token cannot reach is refused before the apply lock is
/// taken, so nothing durable changes and the peer is told forbidden.
#[test]
fn a_push_for_an_envelope_bank_the_token_cannot_reach_is_forbidden() {
    let s = serve_sealed();
    let addr = s.server.addr().to_owned();
    let (status, value) = push(&addr, &s.token, "b-b", &[]);
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "forbidden", "{value}");
    assert_eq!(s.store.journal_max_seq("b-b").unwrap(), 0);
    s.server.stop();
}

/// A body that is not JSON is refused as an argument error, before any bank or
/// mutation is read.
#[test]
fn a_push_body_that_is_not_json_is_refused_locally() {
    let err = super::decode_push("{ not json").expect_err("the body does not parse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("bad push body"),
        "the refusal names the body: {}",
        err.message
    );
}

/// A body whose mutation does not decode is refused as an argument error, so a
/// malformed row never reaches the apply path.
#[test]
fn a_push_mutation_that_does_not_decode_is_refused_locally() {
    let err = super::decode_push(r#"{"bank":"b-a","mutations":[7]}"#)
        .expect_err("a number is not a mutation");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("bad mutation"),
        "the refusal names the mutation: {}",
        err.message
    );
}
