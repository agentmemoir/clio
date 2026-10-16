// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer-evidence coverage of the erasure
//! completion gate, the peer-action reconcile, and the pruning survival of a
//! pending peer action: a settled peer needs its durable acknowledged row or
//! its recorded removal evidence, an approved retention and an unknown
//! disposition keep the completion open, reconciliation settles a pending
//! peer-denial action by action id without an effect, and a completed-event
//! prune never removes the pending peer row.
//!
//! ## Owns
//! - The peer-evidence settle assertions over the durable erasure seam
//! - The retained-result reconcile assertion
//! - The peer-action reconcile and prune-survival assertions
//!
//! ## Does not own
//! - The retry contract (`erasure_retry.rs`), the reconcile
//!   (`erasure_reconcile.rs`), and the durable event writer (`clio-store`)
//! - The closure-fact fold over the durable peer rows
//!   (`peer_row_closure_facts_tests.rs`)
//! - The arms that must stay pending and the restart boundary
//!   (`erasure_retry_peer_pending_tests.rs`)
//! - The shared fixtures (`erasure_retry_tests.rs`,
//!   `subject_request_store_wiring_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a peer without durable evidence settles a completion,
//! when a recorded removal evidence no longer settles, when reconciliation
//! repeats a peer effect, or when pruning drops a pending peer action.
//!
//! Test-only. Never compiled into production builds.

use super::super::subject_request_store_wiring_tests::{
    RetryStore, Scratch, erase_args, open_sqlite, persist, scope_parts, started_record,
    subject_scope, wiring_item,
};
use super::{Attempt, NOW, action_rows, live_set};
use crate::erase::EraseVecSink;
use crate::erasure_reconcile::reconcile_erasure_actions;
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::subject_peer::{
    begin_denial, mark_acknowledged, mark_refused, peer_action_id, record_disposition,
};
use crate::subject_request::PeerState;
use crate::subject_request_events::record_peer_denial_intent;
use crate::subject_request_store_wiring::erase_from_durable_store;
use crate::test_unique::unique;
use clio_store::{
    LocalEventIntent, LocalEventState, LocalEventStore, PeerActionDisposition, PeerActionKind,
    PeerActionRow, PeerActionState, PeerActionStore, SqliteStore, Store,
};
use clio_types::{Actor, Bank, OpContext};
use std::sync::atomic::Ordering;

/// The durable peer row one request and peer currently hold.
fn load_peer_row(store: &SqliteStore, request_id: &str, peer_id: &str) -> PeerActionRow {
    PeerActionStore::load(store, request_id, peer_id, PeerActionKind::Denial)
        .expect("load the peer row")
        .expect("the durable peer row exists")
}

/// Every peer acknowledged durably settles one allowed outcome through the
/// durable seam.
#[test]
fn every_peer_acknowledged_durably_settles_one_allowed_outcome() {
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
        record.peer_banks.insert(peer_bank.clone());
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    begin_denial(&store, &request_id, &peer_bank).expect("the peer row begins");
    mark_acknowledged(&store, &request_id, &peer_bank, "deletion-9")
        .expect("the peer acknowledges");
    let mut double = RetryStore::failing();
    double.fail.store(false, Ordering::SeqCst);
    double.banks = vec![bank.clone(), peer_bank];
    let ctx = OpContext::new(Bank::new(bank), Actor::Harness);
    let sink = EraseVecSink::default();
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let args = erase_args(&double, &ctx, &sink, &request_id, &subject, &salt);
    let live = double.effect_bank_set();
    let outcome = erase_from_durable_store(&store, &store, &store, &request_id, &args, &live)
        .expect("the coordinated erasure completes");
    assert_eq!(outcome.status, "COMPLETED");
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the action is settled"
    );
    let rows = action_rows(&store, &erasure_action_id(&request_id));
    assert_eq!(rows.len(), 1, "one allowed outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
}

/// An approved retention is not removal evidence: the peer keeps an open
/// removal action, so the settle writes no outcome.
#[test]
fn a_retention_approved_peer_keeps_the_completion_open() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.peers.insert(peer_bank.clone(), PeerState::Disposed);
    record.peer_banks.insert(peer_bank.clone());
    persist(&store, &record);
    for bank in [&attempt.bank, &peer_bank] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    record_disposition(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionDisposition::RetentionApproved,
        Some("retention-7"),
    )
    .expect("the retention is recorded");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "an approved retention is not removal evidence"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![erasure_action_id(&attempt.request_id)],
        "the action stays pending"
    );
    let rows = action_rows(&store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
}

/// A recorded removal evidence disposition with its proof reference settles
/// the peer without an acknowledgement state.
#[test]
fn a_removal_evidence_disposition_settles_with_the_proof_reference() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.peers.insert(peer_bank.clone(), PeerState::Disposed);
    record.peer_banks.insert(peer_bank.clone());
    persist(&store, &record);
    for bank in [&attempt.bank, &peer_bank] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    record_disposition(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("the removal evidence is recorded");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(outcome.status, "COMPLETED");
    let rows = action_rows(&store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
}

/// A refused peer row with no disposition is not evidence: the completion
/// stays open until an acknowledgement or a disposition arrives.
#[test]
fn an_unknown_peer_disposition_keeps_the_completion_open() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.peers.insert(peer_bank.clone(), PeerState::Pending);
    record.peer_banks.insert(peer_bank.clone());
    persist(&store, &record);
    for bank in [&attempt.bank, &peer_bank] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    mark_refused(&store, &attempt.request_id, &peer_bank).expect("the refusal records");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a refusal alone is not removal evidence"
    );
    let rows = action_rows(&store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
}

/// Reconciliation settles a pending peer-denial action from its durable
/// acknowledged row by action id, without running an effect.
#[test]
fn reconcile_settles_a_pending_peer_action_by_action_id() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    persist(&store, &started_record(&attempt.request_id, &attempt.scope));
    record_peer_denial_intent(&store, &attempt.request_id, &peer_bank, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    mark_acknowledged(&store, &attempt.request_id, &peer_bank, "deletion-9")
        .expect("the peer acknowledges");
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let report = reconcile_erasure_actions(&store, &store, &env, &|_| None).expect("reconcile");
    let action = peer_action_id(&attempt.request_id, &peer_bank);
    assert_eq!(report.settled, vec![action.clone()], "settled by action id");
    assert!(report.pending.is_empty(), "{report:?}");
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the peer action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "no peer action stays pending"
    );
}

/// Pruning sweeps completed (allowed) events but never a pending peer denial:
/// the row and its pending state survive `prune_completed_before`.
#[test]
fn pruning_keeps_a_pending_peer_action_and_its_denial_state() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let completed = unique("completed-erasure");
    store
        .record_intent(&LocalEventIntent {
            action_id: &completed,
            actor: "harness",
            scope: "prune-scope",
            kind: "erasure",
            event_time: NOW,
        })
        .expect("the completed intent records");
    store
        .record_outcome(&completed, false, "erasure completed")
        .expect("the completed outcome records");
    record_peer_denial_intent(&store, &attempt.request_id, &peer_bank, Actor::Harness, NOW)
        .expect("the peer intent records");
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    let pruned = store
        .prune_completed_before("2026-01-02T00:00:00Z")
        .expect("the prune runs");
    assert_eq!(pruned, 1, "only the completed action is pruned");
    let action = peer_action_id(&attempt.request_id, &peer_bank);
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "the pending peer action survives");
    assert_eq!(
        rows[0].state,
        LocalEventState::Pending,
        "its pending denial state survives"
    );
    let peer_row = PeerActionStore::load(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionKind::Denial,
    )
    .expect("load the peer row")
    .expect("the durable peer row exists");
    assert_eq!(
        peer_row.state,
        PeerActionState::Pending,
        "the durable denial state survives"
    );
}

/// A refused peer row that recorded the peer's removal evidence settles the
/// erasure: an evidenced operator disposition resolves the refusal.
#[test]
fn a_refused_peer_row_resolved_by_removal_evidence_settles_the_erasure() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.peers.insert(peer_bank.clone(), PeerState::Disposed);
    record.peer_banks.insert(peer_bank.clone());
    persist(&store, &record);
    for bank in [&attempt.bank, &peer_bank] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    mark_refused(&store, &attempt.request_id, &peer_bank).expect("the refusal records");
    record_disposition(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("an evidenced disposition resolves the refused row");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "COMPLETED",
        "the evidenced disposition settles the peer"
    );
    let rows = action_rows(&store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows.len(), 1, "one allowed outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "nothing stays pending"
    );
    assert_eq!(
        load_peer_row(&store, &attempt.request_id, &peer_bank).state,
        PeerActionState::Refused,
        "the refusal state is kept"
    );
}

/// Reconciliation settles a retention-approved peer action as an allowed
/// retention, never as a denial.
#[test]
fn reconcile_settles_a_retention_approved_peer_action_as_allowed() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    persist(&store, &started_record(&attempt.request_id, &attempt.scope));
    record_peer_denial_intent(&store, &attempt.request_id, &peer_bank, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    mark_refused(&store, &attempt.request_id, &peer_bank).expect("the refusal records");
    record_disposition(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionDisposition::RetentionApproved,
        Some("hold-7"),
    )
    .expect("the approved retention records");
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let report = reconcile_erasure_actions(&store, &store, &env, &|_| None).expect("reconcile");
    let action = peer_action_id(&attempt.request_id, &peer_bank);
    assert_eq!(
        report.settled,
        vec![action.clone()],
        "the recorded retention settles the action"
    );
    assert!(report.pending.is_empty(), "{report:?}");
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the peer action");
    assert_eq!(
        rows[0].state,
        LocalEventState::Allowed,
        "an approved retention is not a denial"
    );
    assert_eq!(
        rows[0].result.as_deref(),
        Some("peer retention approved"),
        "the retention's own content-free result"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "no peer action stays pending"
    );
    assert_eq!(
        load_peer_row(&store, &attempt.request_id, &peer_bank).state,
        PeerActionState::Refused,
        "the peer row is read, never rewritten"
    );
}

// Declared here, beside the peer fixtures it reuses: the arms of the peer
// completion gate that must stay pending, the restart boundary, and the
// peer-denial reconcile arms over a reopened store.
#[path = "erasure_retry_peer_pending_tests.rs"]
mod erasure_retry_peer_pending_tests;
