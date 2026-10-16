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

use super::erasure_retry_tests::{Attempt, live_set, load_row, tombstone_count};
use super::subject_request_store_wiring_tests::{
    RetryStore, Scratch, erase_args, open_sqlite, persist, scope_parts, started_record,
    subject_scope, wiring_item,
};
use crate::erase::EraseVecSink;
use crate::erasure_retry::erasure_action_id;
use crate::subject_request::{PeerState, RequestKind, RequestScope};
use crate::subject_request_store_wiring::{erase_from_durable_store, record_to_row, row_to_record};
use crate::test_unique::unique;
use clio_store::{ComplianceStore, DurableRequestStore, LocalEventState, LocalEventStore, Store};
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
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &store, &request_id, &args, &live)
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

/// A subject-bank write landing after the confirmation widens the live bank
/// set but can never widen the confirmed erasure: the seam refuses before any
/// intent event, journal denial, key loss, or store effect.
#[test]
fn sqlite_a_new_subject_bank_write_after_confirmation_cannot_widen_the_erasure() {
    let attempt = Attempt::new();
    let new_bank = unique("new-bank");
    let original = wiring_item(&attempt.bank, &attempt.subject);
    let new_item = wiring_item(&new_bank, &attempt.subject);
    let store = open_sqlite(&attempt.store_path());
    store
        .create_memory_item(&original, "tester")
        .expect("seed the confirmed bank");
    persist(&store, &started_record(&attempt.request_id, &attempt.scope));
    store
        .create_memory_item(&new_item, "tester")
        .expect("the new subject-bank write lands");
    let before = load_row(&store, &attempt.request_id);
    let live = live_set(&attempt, &store);
    assert_eq!(live.len(), 2, "the new write widens the live set: {live:?}");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let err = erase_from_durable_store(&store, &store, &attempt.request_id, &args, &live)
        .expect_err("a widened bank set must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        store.list_events().expect("events").is_empty(),
        "the refusal must precede any intent event"
    );
    assert!(!attempt.journal.exists(), "the refusal precedes any denial");
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the refusal loses no key"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        0,
        "the refusal writes no tombstone"
    );
    assert_eq!(
        load_row(&store, &attempt.request_id),
        before,
        "the refusal must leave the durable row untouched"
    );
    assert!(
        store
            .get_memory_item(&attempt.bank, &original.id)
            .expect("readable")
            .is_some(),
        "the confirmed bank's item survives untouched"
    );
    assert!(
        store
            .get_memory_item(&new_bank, &new_item.id)
            .expect("readable")
            .is_some(),
        "the newly written bank's item survives untouched"
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
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let first = erase_from_durable_store(&store, &store, &request_id, &args, &live)
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
    let retry = erase_from_durable_store(&store, &store, &request_id, &args, &live)
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
    let recovered = erase_from_durable_store(&store, &store, &request_id, &args, &live)
        .expect("the recovered attempt completes");
    assert_eq!(recovered.status, "COMPLETED");
    assert_eq!(double.erase_calls(), 3, "each attempt runs the effect once");
}

/// A row whose kind column disagrees with its record fails closed.
#[test]
fn sqlite_a_row_with_a_drifted_kind_column_fails_closed() {
    let record = started_record(&unique("req"), &subject_scope());
    let mut row = record_to_row(&record);
    row.kind = serde_json::to_string(&RequestKind::Copy).expect("encode kind");
    assert_eq!(row_to_record(&row).unwrap_err().code, ErrorCode::Internal);
}

/// A row whose scope column disagrees with its record fails closed.
#[test]
fn sqlite_a_row_with_a_drifted_scope_column_fails_closed() {
    let record = started_record(&unique("req"), &subject_scope());
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
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &store, &request_id, &args, &live)
        .expect_err("a missing durable row must refuse");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        double.erase_calls(),
        0,
        "the refusal must precede any effect"
    );
}

/// An erasure whose durable row no longer decodes fails closed before any
/// erasure effect.
#[test]
fn sqlite_an_erasure_with_a_corrupted_durable_row_fails_closed() {
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
    let mut corrupted = store
        .load(&request_id)
        .expect("load the row")
        .expect("row exists");
    corrupted.record = "{not json".into();
    store
        .save(&corrupted)
        .expect("overwrite with the corrupted record");
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone()];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &store, &request_id, &args, &live)
        .expect_err("a corrupted durable row must refuse");
    assert_eq!(err.code, ErrorCode::Internal);
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

/// Malformed stored JSON fails closed instead of restoring a partial record.
#[test]
fn a_row_with_invalid_record_json_fails_closed() {
    let record = started_record("req", &subject_scope());
    let mut row = record_to_row(&record);
    row.record = "{".into();
    let err = row_to_record(&row).expect_err("invalid JSON must refuse");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("durable request record decode"));
}

/// A row cannot restore a record saved under another request id.
#[test]
fn a_row_with_another_request_id_fails_closed() {
    let record = started_record("req", &subject_scope());
    let mut row = record_to_row(&record);
    row.request_id = "other-req".into();
    let err = row_to_record(&row).expect_err("mismatched request id must refuse");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("holds the record of"));
}

/// A missing durable confirmation refuses before any erasure effect.
#[test]
fn sqlite_a_missing_request_refuses_before_any_erasure_effect() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let double = RetryStore::failing();
    let ctx = OpContext::new(Bank::new("bank"), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, "missing", "subject", &salt);
    let err = erase_from_durable_store(&store, &store, "missing", &args, &[])
        .expect_err("missing confirmation must refuse");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(double.erase_calls(), 0, "no confirmation permits no effect");
}

/// A pending peer refuses the seam before any intent event, store effect, or
/// key loss.
#[test]
fn sqlite_a_pending_peer_refuses_before_any_intent_event() {
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
    let mut double = RetryStore::failing();
    double.banks = vec![bank.clone(), peer_bank];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let err = erase_from_durable_store(&store, &store, &request_id, &args, &live)
        .expect_err("a pending peer must refuse the preflight");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        store.list_events().expect("events").is_empty(),
        "the refusal writes no intent event"
    );
    assert_eq!(double.erase_calls(), 0, "the refusal runs no store effect");
    assert!(
        !double.subject_dek_destroyed(&subject).expect("key"),
        "the refusal loses no key"
    );
}

/// Every peer acknowledged settles one allowed outcome through the seam.
#[test]
fn sqlite_every_peer_acknowledged_settles_one_allowed_outcome() {
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
    let mut double = RetryStore::failing();
    double.fail.store(false, Ordering::SeqCst);
    double.banks = vec![bank.clone(), peer_bank];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let outcome = erase_from_durable_store(&store, &store, &request_id, &args, &live)
        .expect("the coordinated erasure completes");
    assert_eq!(outcome.status, "COMPLETED");
    let action_id = erasure_action_id(&request_id);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the action is settled"
    );
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    assert_eq!(rows.len(), 1, "one allowed outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
}
