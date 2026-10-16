// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's denial surface over real TCP: the pull
//! refusal for a bank a recorded denial covers while an unrelated bank still
//! serves, the refusal of a pull whose authority is unreadable, the durable
//! proof a push acknowledgement carries for a denial operation, its
//! redelivery, and the refusal a restarted server still reports.
//!
//! ## Owns
//! - Behavior coverage for the pull-side bank guard and the push-side proof
//!
//! ## Does not own
//! - The apply-side guards at the engine level (`apply_denial_tests`)
//! - The wire vocabulary in isolation (`denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the routes' denial behavior changes; they never
//! change it.
//!
//! Keep this module focused on the server's denial surface.

use crate::credential_fixtures::{Scratch, record, secret};
use crate::denial::{DenialCarrier, DenialProof, DenialProofBinding, denial_mutation};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{SyncContent, SyncMutation};
use crate::server::server_tests::{http, item_value, mutation, push_body, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{DekProvider, ErasureDenial, SqliteStore, Store, SyncStore};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// Server options over `store` and the credential store at `store_path`.
fn options(
    store: &Arc<SqliteStore>,
    kms: Arc<dyn DekProvider>,
    store_path: &Path,
) -> SyncServerOptions {
    SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: Arc::clone(store) as Arc<dyn Store>,
        sync: Arc::clone(store) as Arc<dyn SyncStore>,
        kms,
        banks: vec!["b-a".into(), "b-b".into(), "b-c".into()],
        token_file: Some(store_path.to_path_buf()),
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
    }
}

/// A scoped credential for all three banks, and the token it presents.
fn credential() -> (Scratch, String) {
    let scratch = Scratch::new("denial-serve");
    let token = secret("t");
    scratch.write(&[record("client-a", &token, &["b-a", "b-b", "b-c"])]);
    (scratch, token)
}

/// A running server with the authority wired, its store, the authority, and
/// the credential the tests present.
fn serve_denied() -> (
    SyncServer,
    Arc<SqliteStore>,
    Arc<FakeDenial>,
    Scratch,
    String,
) {
    let (scratch, token) = credential();
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let server = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, store, denial, scratch, token)
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

/// One pull of `bank`.
fn pull(addr: &str, token: &str, bank: &str) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/pull",
        Some(token),
        Some(&json!({ "bank": bank, "cursor": "", "limit": 10 })),
    )
}

/// The one denial operation a request sends, covering `banks` and riding `bank`.
fn denial_op(bank: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        "s-held",
        &DenialCarrier::new("req-1", banks),
        "dev-a",
        "2026-01-01T00:00:00Z",
    )
}

#[test]
fn a_denied_bank_is_refused_while_an_unrelated_bank_serves() {
    let (server, _store, _denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    // One row per bank, pushed before the denial. Item ids are store-wide, so
    // each bank gets its own.
    for (bank, item, mid) in [("b-a", "i-a", "m-a"), ("b-c", "i-c", "m-c")] {
        let mut m = mutation(mid, bank, "2026-01-01T00:00:00Z", "g");
        m.entity_id = item.into();
        m.content = SyncContent::Plain(item_value(bank, item, "2026-01-01T00:00:00Z", "g"));
        let (status, value) = push(&addr, &token, bank, &[m]);
        assert_eq!(status, 200, "push {bank}: {value}");
    }
    let (status, ack) = push(&addr, &token, "b-a", &[denial_op("b-a", &["b-a", "b-b"])]);
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["denial:req-1"]));

    let (status, value) = pull(&addr, &token, "b-a");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");
    let (status, value) = pull(&addr, &token, "b-b");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");

    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 200, "{value}");
    assert_eq!(
        value["mutations"].as_array().expect("mutations").len(),
        1,
        "an unrelated bank's rows are served: {value}"
    );
    server.stop();
}

#[test]
fn an_unreadable_authority_refuses_the_pull_for_an_unrelated_bank() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 200, "{value}");
    denial.damage();
    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 500, "{value}");
    assert_eq!(value["code"], "config_corrupt");
    server.stop();
}

#[test]
fn a_push_acknowledges_a_denial_with_its_durable_proof() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", &["b-a", "b-b"]);
    let (status, ack) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    assert_eq!(status, 200, "{ack}");
    let acks = ack["denial_acks"].as_array().expect("denial acks");
    assert_eq!(acks.len(), 1, "{ack}");
    assert_eq!(acks[0]["sync_mutation_id"], "denial:req-1");
    let proof: DenialProof =
        serde_json::from_value(acks[0]["proof"].clone()).expect("proof parses");
    assert_eq!(proof.request_ref, "req-1");
    assert_eq!(proof.peer_id, "srv-1");
    assert_eq!(proof.generation, denial.generation());
    assert_eq!(denial.records_for("s-held"), 1, "one durable record");

    // The proof verifies against this deployment's own fingerprint and binds
    // the request, the peer, and the subject; a proof answered under another
    // fingerprint is refused rather than acknowledged.
    let carrier = DenialCarrier::new("req-1", &["b-a", "b-b"]);
    let binding = DenialProofBinding::for_carrier(&op, &carrier, "srv-1", denial.fingerprint());
    assert_eq!(proof.verify(&binding), Ok(()));
    let elsewhere = DenialProofBinding::for_carrier(&op, &carrier, "srv-1", "fp-other");
    assert!(proof.verify(&elsewhere).is_err());
    server.stop();
}

#[test]
fn a_redelivered_denial_push_returns_the_same_proof() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", &["b-a"]);
    let (_, first) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    let (_, second) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    assert_eq!(second["accepted"], json!(["denial:req-1"]));
    assert_eq!(
        second["denial_acks"], first["denial_acks"],
        "a redelivery answers with the same durable proof"
    );
    assert_eq!(denial.records_for("s-held"), 1, "one durable record");
    server.stop();
}

#[test]
fn a_restarted_server_still_refuses_a_denied_bank() {
    let (scratch, token) = credential();
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let first = sync_serve_with_denial(
        options(&store, Arc::clone(&kms), scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let (status, ack) = push(first.addr(), &token, "b-a", &[denial_op("b-a", &["b-a"])]);
    assert_eq!(status, 200, "{ack}");
    first.stop();

    // A fresh server over the same store and the same authority: the refusal
    // comes from the authority, not from process state.
    let second = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let (status, value) = pull(second.addr(), &token, "b-a");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");
    second.stop();
}
