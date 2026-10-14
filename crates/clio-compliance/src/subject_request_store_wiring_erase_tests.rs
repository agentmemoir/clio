// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable erasure seam coverage on
//! SQLite: an incomplete or stale confirmation refusing before any erasure
//! effect, a partial erasure retried through the durable store never reporting
//! completion early, the drifted-row refusal, and the fan-out bank set.
//!
//! ## Owns
//! - The durable-store erasure refusals and the PROCESSING retry assertions
//! - The drifted-row refusal and the fan-out bank set the seam reads
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The coordination refusals over an in-memory record
//!   (`subject_bank_coordination_tests.rs`)
//! - The shared fixtures and the shared erase double
//!   (`subject_request_store_wiring_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the durable erasure seam stops refusing before any
//! effect or reports a completion the store never reached.
//!
//! Test-only. Never compiled into production builds.

use super::subject_request_store_wiring_tests::{
    RetryStore, Scratch, erase_args, open_sqlite, persist, scope_parts, started_record,
    subject_scope, wiring_item,
};
use crate::erase::EraseVecSink;
use crate::subject_request::{PeerState, RequestKind, RequestScope};
use crate::subject_request_store_wiring::{erase_from_durable_store, record_to_row, row_to_record};
use crate::test_unique::unique;
use clio_store::{DurableRequestStore, Store};
use clio_types::{Actor, Bank, ErrorCode, OpContext};
use std::sync::atomic::Ordering;

/// A peer bank that has not acknowledged refuses the durable-store erasure
/// before any erasure effect.
#[test]
fn sqlite_an_incomplete_confirmation_refuses_before_any_erasure_effect() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    let (bank, subject) = scope_parts(&scope);
    let peer_bank = unique("peer-bank");
    {
        let store = open_sqlite(&path);
        let mut record = started_record(&request_id, &scope);
        record.peers.insert(peer_bank.clone(), PeerState::Pending);
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let before = store
        .load(&request_id)
        .expect("load the row")
        .expect("row exists");
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone(), peer_bank];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect_err("an unacknowledged peer must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        double.erase_calls(),
        0,
        "the refusal must precede any effect"
    );
    assert_eq!(
        store
            .load(&request_id)
            .expect("load the row")
            .expect("row exists"),
        before,
        "the refusal must leave the durable row untouched"
    );
}

/// A bank set that changed after confirmation refuses the durable-store
/// erasure after a restart too.
#[test]
fn sqlite_a_bank_set_changed_after_confirmation_refuses() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    let (bank, subject) = scope_parts(&scope);
    let peer_bank = unique("peer-bank");
    {
        let store = open_sqlite(&path);
        let mut record = started_record(&request_id, &scope);
        record
            .peers
            .insert(peer_bank.clone(), PeerState::Acknowledged);
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let before = store
        .load(&request_id)
        .expect("load the row")
        .expect("row exists");
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone(), peer_bank, unique("joined-bank")];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect_err("a changed bank set must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        double.erase_calls(),
        0,
        "the refusal must precede any effect"
    );
    assert_eq!(
        store
            .load(&request_id)
            .expect("load the row")
            .expect("row exists"),
        before,
        "the refusal must leave the durable row untouched"
    );
}

/// A partial erasure retried through the durable store stays `PROCESSING`
/// while the purge is incomplete and reports `COMPLETED` only once it
/// succeeds.
#[test]
fn sqlite_a_partial_erasure_retry_never_reports_completion_early() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    let (bank, subject) = scope_parts(&scope);
    {
        let store = open_sqlite(&path);
        persist(&store, &started_record(&request_id, &scope));
    }
    let store = open_sqlite(&path);
    let before = store
        .load(&request_id)
        .expect("load the row")
        .expect("row exists");
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone()];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject);
    let live = double.effect_bank_set();
    let first = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect("a partial failure reports PROCESSING");
    assert_eq!(first.status, "PROCESSING");
    assert_eq!(
        double.erase_calls(),
        1,
        "the first attempt runs the effect once"
    );
    assert_eq!(
        store
            .load(&request_id)
            .expect("load the row")
            .expect("row exists"),
        before,
        "an attempt must not rewrite the durable row"
    );
    let retry = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect("the retried attempt also reports PROCESSING");
    assert_eq!(
        retry.status, "PROCESSING",
        "a retry must not report false completion"
    );
    assert_eq!(double.erase_calls(), 2, "each attempt runs the effect once");
    assert_eq!(
        store
            .load(&request_id)
            .expect("load the row")
            .expect("row exists"),
        before,
        "a retry reloads the same row"
    );
    double.fail.store(false, Ordering::SeqCst);
    let recovered = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect("the recovered attempt completes");
    assert_eq!(recovered.status, "COMPLETED");
    assert_eq!(double.erase_calls(), 3, "each attempt runs the effect once");
}

/// A row whose kind or scope column disagrees with its record fails closed.
#[test]
fn sqlite_a_row_with_a_drifted_kind_or_scope_column_fails_closed() {
    let scope = subject_scope();
    let record = started_record(&unique("req"), &scope);
    let mut row = record_to_row(&record);
    row.kind = serde_json::to_string(&RequestKind::Copy).expect("encode kind");
    assert_eq!(row_to_record(&row).unwrap_err().code, ErrorCode::Internal);
    let mut row = record_to_row(&record);
    row.scope = serde_json::to_string(&RequestScope::Bank {
        bank_id: unique("wiring-bank"),
    })
    .expect("encode scope");
    assert_eq!(row_to_record(&row).unwrap_err().code, ErrorCode::Internal);
}

/// An erasure for a request the durable store does not hold refuses before
/// any erasure effect.
#[test]
fn sqlite_an_erasure_for_a_request_the_store_does_not_hold_refuses() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let request_id = unique("req");
    let scope = subject_scope();
    let (bank, subject) = scope_parts(&scope);
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone()];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &request_id, &args, &live)
        .expect_err("a missing durable row must refuse");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        double.erase_calls(),
        0,
        "the refusal must precede any effect"
    );
}

/// The store reports the fan-out set: the requesting bank plus every bank
/// holding the subject's items.
#[test]
fn sqlite_the_store_reports_the_erasure_fan_out_bank_set() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let store = open_sqlite(&path);
    let bank = unique("wiring-bank");
    let other_bank = unique("wiring-bank");
    let subject = unique("subject");
    store
        .create_memory_item(&wiring_item(&bank, &subject), "tester")
        .expect("write the subject item");
    store
        .create_memory_item(&wiring_item(&other_bank, &subject), "tester")
        .expect("write the peer-bank item");
    let mut expected = vec![bank, other_bank];
    expected.sort();
    assert_eq!(
        store
            .erase_bank_set(&expected[0], &subject)
            .expect("read the fan-out set"),
        expected,
        "the live set must cover every bank sharing the key"
    );
}
