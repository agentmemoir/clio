// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pull-side trust gate on a denial carrier: a carrier
//! received from a peer the operator has not trusted records nothing and is
//! quarantined, while the same carrier from a trusted peer records its bank
//! marker and subject denial and is journaled for relay.
//!
//! ## Owns
//! - The untrusted-peer refusal: no marker, no subject denial, no journal row,
//!   no cursor advance, and a visible dead letter
//! - The trusted-peer acceptance: both durable records and the journal row
//!
//! ## Does not own
//! - The push-side credential gate (`server_push_denial_tests`)
//! - The engine-level carrier guards (`apply_denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the pull carrier authority changes; they never change
//! it.
//!
//! Keep this module focused on the pull-side trust decision.

use crate::client::client_tests::paired_key;
use crate::client::{SyncClientConfig, pull_with_denial};
use crate::credential_fixtures::{Scratch, record_as, secret};
use crate::denial::{DenialCarrier, denial_mutation, denied_bank_marker};
use crate::denial_fixtures::FakeDenial;
use crate::server::server_tests::{http, push_body, sqlite};
use crate::server::{SyncServerOptions, sync_serve_with_denial};
use clio_store::{DeadLetterStatus, ErasureDenial, SqliteStore, Store, SyncStore};
use clio_types::Actor;
use serde_json::json;
use std::sync::Arc;

const BANK: &str = "b-a";
const COVERED: &str = "b-b";

/// A server holding one harness credential, so a denial push is accepted.
fn serve() -> (crate::server::SyncServer, Arc<SqliteStore>, String, Scratch) {
    let scratch = Scratch::new("pull-carrier-trust");
    let token = secret("pull-trust");
    scratch.write(&[record_as(
        "trusted-peer",
        &token,
        Actor::Harness,
        &[BANK, COVERED],
    )]);
    let (store, kms) = sqlite();
    let denial: Arc<dyn ErasureDenial> = FakeDenial::new("fp-serve");
    let server = sync_serve_with_denial(
        SyncServerOptions {
            bind: "127.0.0.1:0".into(),
            store: Arc::clone(&store) as Arc<dyn Store>,
            sync: Arc::clone(&store) as Arc<dyn SyncStore>,
            kms,
            banks: vec![BANK.into(), COVERED.into()],
            token_file: Some(scratch.path().to_path_buf()),
            dev_mode: false,
            allow_public_bind: false,
            ttl_secs: None,
            deadline_secs: None,
            max_envelope_skew_secs: None,
            device_id: "srv-1".into(),
            sync_key: Some(Arc::new(paired_key())),
        },
        denial,
    )
    .expect("serve");
    (server, store, token, scratch)
}

/// Seed the server journal with one denial carrier under a harness token.
fn seed_carrier(addr: &str, token: &str) {
    let op = denial_mutation(
        BANK,
        "s-held",
        &DenialCarrier::new("req-pull-trust", &[COVERED]),
        "dev-a",
        "2026-01-01T00:00:00Z",
    );
    let content = op.content_value().clone();
    let op = crate::envelope_fixtures::seal_content_for(op, &paired_key(), &content);
    let (status, ack) = http(
        addr,
        "POST",
        "/sync/push",
        Some(token),
        Some(&push_body(BANK, &[op])),
    );
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["denial:req-pull-trust"]), "{ack}");
}

/// A client config for `addr` under `token`, with the trust decision set.
fn client(addr: &str, token: &str, trusted: bool) -> SyncClientConfig {
    let (store, _kms) = sqlite();
    SyncClientConfig {
        device_id: "dev-b".into(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: store as Arc<dyn SyncStore>,
        remote: Some(addr.to_owned()),
        token: Some(token.to_owned()),
        cipher: vec![paired_key()],
        allow_plaintext: false,
        max_envelope_skew_secs: crate::ENVELOPE_SKEW_DEFAULT_SECS,
        trust_erasure_carriers: trusted,
    }
}

#[test]
fn a_carrier_from_an_untrusted_peer_records_nothing() {
    let (server, _store, token, _scratch) = serve();
    let addr = server.addr().to_owned();
    seed_carrier(&addr, &token);
    let cfg = client(&addr, &token, false);
    let denial = FakeDenial::new("fp-pull");
    let rows =
        pull_with_denial(&cfg, &[BANK.to_owned()], Some(denial.as_ref())).expect("pull runs");
    assert_eq!(rows.len(), 1, "one bank reported");

    assert_eq!(
        denial.records_for(&denied_bank_marker(COVERED)),
        0,
        "an untrusted peer records no bank marker"
    );
    assert_eq!(
        denial.records_for("s-held"),
        0,
        "an untrusted peer records no subject denial"
    );
    assert_eq!(
        cfg.sync
            .dead_letter_status("denial:req-pull-trust")
            .unwrap(),
        Some(DeadLetterStatus::Open),
        "the refusal is quarantined for the operator"
    );
    assert!(
        cfg.sync
            .journal_get(BANK, "denial:req-pull-trust")
            .unwrap()
            .is_none(),
        "a refused carrier is never journaled"
    );
    assert_eq!(
        cfg.sync.cursor_get(BANK, "pull").unwrap(),
        None,
        "the cursor never advances past the refusal"
    );
    server.stop();
}

#[test]
fn a_carrier_from_a_trusted_peer_records_the_denial() {
    let (server, _store, token, _scratch) = serve();
    let addr = server.addr().to_owned();
    seed_carrier(&addr, &token);
    let cfg = client(&addr, &token, true);
    let denial = FakeDenial::new("fp-pull");
    let rows =
        pull_with_denial(&cfg, &[BANK.to_owned()], Some(denial.as_ref())).expect("pull runs");
    assert_eq!(rows.len(), 1, "one bank reported");

    assert_eq!(
        denial.records_for(&denied_bank_marker(COVERED)),
        1,
        "the trusted peer's bank marker is durable"
    );
    assert_eq!(
        denial.records_for("s-held"),
        1,
        "the trusted peer's subject denial is durable"
    );
    assert!(
        cfg.sync
            .journal_get(BANK, "denial:req-pull-trust")
            .unwrap()
            .is_some(),
        "the accepted carrier is journaled for relay"
    );
    assert!(
        cfg.sync.cursor_get(BANK, "pull").unwrap().is_some(),
        "an applied page advances the cursor"
    );
    server.stop();
}
