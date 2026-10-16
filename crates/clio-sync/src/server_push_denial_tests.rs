// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the denial behavior of the sync push and pull routes: a
//! push credential may not record serve markers for a bank it cannot reach,
//! an authorized carrier still records every covered bank's marker, a push
//! refused by an unreadable authority answers with that refusal's own code,
//! and a pulled row for a denied subject is dead-lettered while a clear row in
//! the same page applies.
//!
//! ## Owns
//! - Behavior coverage for the push route's covered-bank authorization and its
//!   coded 500 refusal
//! - Behavior coverage for the guarded client pull (`pull_with_denial`): a
//!   denied subject before any write, a clear row still applied
//!
//! ## Does not own
//! - The pull-side bank guard and the proof round trip (`server_denial_tests`)
//! - The engine-level guards (`apply_denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the routes' denial refusals change; they never
//! change them.
//!
//! Keep this module focused on the routes' denial refusals.

use crate::apply::apply_tests::{env, item_value};
use crate::client::{SyncClientConfig, pull_with_denial};
use crate::credential_fixtures::{Scratch, record, secret};
use crate::denial::{DenialCarrier, denial_mutation, denied_bank_marker};
use crate::denial_fixtures::FakeDenial;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncMutation};
use crate::server::server_tests::{http, mutation, push_body, serve, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{DeadLetterStatus, ErasureDenial, SqliteStore, Store, SyncStore};
use serde_json::{Value, json};
use std::sync::Arc;

/// A running server with the authority wired and two credentials: one scoped
/// to `b-a` alone, one covering both banks.
fn serve_scoped() -> (
    SyncServer,
    Arc<SqliteStore>,
    Arc<FakeDenial>,
    Scratch,
    String,
    String,
) {
    let scratch = Scratch::new("push-carrier-scope");
    let token_narrow = secret("n");
    let token_wide = secret("w");
    scratch.write(&[
        record("client-narrow", &token_narrow, &["b-a"]),
        record("client-wide", &token_wide, &["b-a", "b-b"]),
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
            ttl_secs: None,
            deadline_secs: None,
            device_id: "srv-1".into(),
        },
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, store, denial, scratch, token_narrow, token_wide)
}

/// The one denial operation a request sends for `subject`, covering `banks`
/// and riding `bank`.
fn denial_op(bank: &str, subject: &str, request_ref: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        subject,
        &DenialCarrier::new(request_ref, banks),
        "dev-a",
        "2026-01-01T00:00:00Z",
    )
}

/// One push of `mutations` under `bank` with `token`.
fn push(addr: &str, token: &str, bank: &str, mutations: &[SyncMutation]) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/push",
        Some(token),
        Some(&push_body(bank, mutations)),
    )
}

/// One pull of `bank` with `token`.
fn pull(addr: &str, token: &str, bank: &str) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/pull",
        Some(token),
        Some(&json!({ "bank": bank, "cursor": "", "limit": 10 })),
    )
}

/// A credential for `b-a` alone cannot record the serve marker of `b-b`: the
/// carrier is refused whole before any marker exists, so one bank's token can
/// never durably refuse another bank's reads.
#[test]
fn a_credential_cannot_record_a_marker_for_a_bank_it_cannot_reach() {
    let (server, store, denial, _scratch, token_narrow, token_wide) = serve_scoped();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", "s-held", "req-1", &["b-a", "b-b"]);
    let (status, ack) = push(&addr, &token_narrow, "b-a", std::slice::from_ref(&op));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(
        ack["rejected"],
        json!(["denial:req-1"]),
        "the carrier is refused: {ack}"
    );
    assert_eq!(ack["accepted"], json!([]));
    assert!(
        ack.get("denial_acks").is_none(),
        "no proof rides a refusal, and the empty list is omitted: {ack}"
    );
    assert_eq!(
        denial.records_for(&denied_bank_marker("b-b")),
        0,
        "no marker for the bank the credential cannot reach"
    );
    assert_eq!(
        denial.records_for(&denied_bank_marker("b-a")),
        0,
        "a refused carrier records no marker at all"
    );
    assert_eq!(denial.records_for("s-held"), 0, "no subject denial either");
    assert_eq!(
        store
            .dead_letter_count("b-a", DeadLetterStatus::Open)
            .unwrap(),
        1,
        "the refusal is quarantined under the authorized envelope bank"
    );
    let (status, page) = pull(&addr, &token_wide, "b-b");
    assert_eq!(status, 200, "b-b still serves its readers: {page}");
    server.stop();
}

/// A carrier whose every covered bank is authorized records one durable marker
/// per bank and one subject denial.
#[test]
fn an_authorized_carrier_records_every_covered_banks_marker() {
    let (server, _store, denial, _scratch, token_narrow, _token_wide) = serve_scoped();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", "s-held", "req-2", &["b-a"]);
    let (status, ack) = push(&addr, &token_narrow, "b-a", std::slice::from_ref(&op));
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["denial:req-2"]), "{ack}");
    assert_eq!(
        denial.records_for(&denied_bank_marker("b-a")),
        1,
        "the covered bank's marker is durable"
    );
    assert_eq!(
        denial.records_for("s-held"),
        1,
        "the subject denial is durable"
    );
    server.stop();
}

/// A push the authority refuses mid-batch answers with the refusal's own code
/// and message, so a client can tell a damaged authority (`config_corrupt`)
/// from a transient storage failure; nothing is applied behind the refusal.
#[test]
fn a_push_refused_by_an_unreadable_authority_answers_its_own_code() {
    let (server, store, denial, _scratch, _token_narrow, token_wide) = serve_scoped();
    let addr = server.addr().to_owned();
    denial.damage();
    let item = mutation("m-refused", "b-a", "2026-01-01T00:00:00Z", "g");
    let (status, value) = push(&addr, &token_wide, "b-a", std::slice::from_ref(&item));
    assert_eq!(status, 500, "{value}");
    assert_eq!(
        value["code"], "config_corrupt",
        "the authority refusal keeps its code: {value}"
    );
    assert_eq!(
        value["message"], "the local erasure-denial authority is unreadable; refusing the mutation",
        "{value}"
    );
    assert!(
        store.journal_get("b-a", "m-refused").unwrap().is_none(),
        "a refusal writes nothing durably"
    );
    server.stop();
}

/// A pulled row for a denied subject is dead-lettered before any write, while
/// a clear row in the same page still applies and the pull cursor does not
/// advance past the refusal.
#[test]
fn a_pulled_denied_subject_is_refused_while_a_clear_row_in_the_batch_applies() {
    let (server, _store) = serve(&["b-a"], Some("tok-plain"), false, None, None);
    let addr = server.addr().to_owned();
    let clear = env(
        "m-clear",
        "b-a",
        ENTITY_KIND_ITEM,
        "i-clear",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        item_value("b-a", "i-clear", "s-clear", "2026-01-01T00:00:00Z", "clear"),
    );
    let held = env(
        "m-held",
        "b-a",
        ENTITY_KIND_ITEM,
        "i-held",
        OP_UPSERT,
        "2026-01-01T00:00:01Z",
        "dev-a",
        item_value("b-a", "i-held", "s-held", "2026-01-01T00:00:01Z", "held"),
    );
    let (status, ack) = push(&addr, "tok-plain", "b-a", &[clear, held]);
    assert_eq!(status, 200, "{ack}");
    assert_eq!(
        ack["accepted"].as_array().expect("accepted").len(),
        2,
        "the serving peer knows no denial: {ack}"
    );

    let (client_store, _kms) = sqlite();
    let cfg = SyncClientConfig {
        device_id: "dev-a".into(),
        store: Arc::clone(&client_store) as Arc<dyn Store>,
        sync: client_store as Arc<dyn SyncStore>,
        remote: Some(addr),
        token: Some("tok-plain".into()),
        cipher: Vec::new(),
    };
    let denial = FakeDenial::new("fp-1");
    denial.deny("s-held", &["b-a"]);
    let rows =
        pull_with_denial(&cfg, &["b-a".to_owned()], Some(denial.as_ref())).expect("pull runs");
    assert_eq!(rows.len(), 1, "one bank reported");

    assert!(
        cfg.store
            .get_memory_item("b-a", "i-clear")
            .unwrap()
            .is_some(),
        "the clear row in the same page applies"
    );
    assert!(
        cfg.store
            .get_memory_item("b-a", "i-held")
            .unwrap()
            .is_none(),
        "the denied row writes no content"
    );
    assert_eq!(
        cfg.sync.dead_letter_status("m-held").unwrap(),
        Some(DeadLetterStatus::Open),
        "the denied row is dead-lettered"
    );
    assert!(
        cfg.sync.journal_get("b-a", "m-held").unwrap().is_none(),
        "a refused row is never journaled"
    );
    assert_eq!(
        cfg.sync.cursor_get("b-a", "pull").unwrap(),
        None,
        "the pull cursor never advances past a refusal"
    );
    server.stop();
}
