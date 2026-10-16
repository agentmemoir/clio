// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the arms of the peer completion gate that
//! must refuse: a delivered row, a removal evidence without its proof
//! reference, a second peer whose own row is unsettled while another peer
//! settled, and the restart boundary over a pending or delivered row, where
//! the erasure entry and the restart reconciliation both keep the action open
//! and repeat no store work.
//!
//! ## Owns
//! - The delivered-row and unproven-removal completion-gate negatives
//! - The two-peer negative over one acknowledged and one unsettled peer row
//! - The restart-over-an-unsettled-peer assertions and their shared body
//! - The peer-denial reconcile arms over a reopened store
//!
//! ## Does not own
//! - The retry contract (`erasure_retry.rs`), the reconcile
//!   (`erasure_reconcile.rs`), and the durable event writer (`clio-store`)
//! - The peer-evidence settle and fold assertions
//!   (`erasure_retry_peer_evidence_tests.rs`) and the shared fixtures
//!   (`erasure_retry_tests.rs`, `subject_request_store_wiring_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a delivered or unproven peer row settles a
//! completion, when one peer's own evidence settles another peer's row, when a
//! restart repeats a purge, or when reconciliation claims a result for a
//! missing, pending, or delivered peer row.
//!
//! Test-only. Never compiled into production builds.

use super::super::super::subject_request_store_wiring_tests::{
    open_sqlite, persist, started_record, wiring_item,
};
use super::super::{Attempt, action_rows, live_set, tombstone_count};
use super::load_peer_row;
use crate::erase::EraseVecSink;
use crate::erasure_reconcile::reconcile_erasure_actions;
use crate::erasure_retry::{erase_confirmed_request, erasure_action_id};
use crate::subject_peer::{
    begin_denial, mark_acknowledged, mark_delivered, peer_action_id, record_disposition,
};
use crate::subject_request::PeerState;
use crate::subject_request_events::record_peer_denial_intent;
use crate::test_unique::unique;
use clio_store::{
    LocalEventState, LocalEventStore, PeerActionDisposition, PeerActionState, PeerActionStore,
    SqliteStore, Store,
};
use clio_types::Actor;

/// A peer slot recorded as pending on the durable record.
fn record_with_a_pending_peer(attempt: &Attempt, peer_bank: &str) -> SqliteStore {
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record
        .peers
        .insert(peer_bank.to_owned(), PeerState::Pending);
    record.peer_banks.insert(peer_bank.to_owned());
    persist(&store, &record);
    for bank in [&attempt.bank, peer_bank] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    store
}

/// A delivered peer row is not removal evidence: the completion stays open,
/// the row keeps its delivery, and no purge repeats.
#[test]
fn a_delivered_peer_row_keeps_the_completion_open_and_the_row_delivered() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = record_with_a_pending_peer(&attempt, &peer_bank);
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    mark_delivered(&store, &attempt.request_id, &peer_bank).expect("the delivery records");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a delivered row is not removal evidence"
    );
    let action = erasure_action_id(&attempt.request_id);
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action.clone()],
        "the action stays pending"
    );
    let peer_row = load_peer_row(&store, &attempt.request_id, &peer_bank);
    assert_eq!(
        peer_row.state,
        PeerActionState::Delivered,
        "the delivery is kept"
    );
    assert_eq!(peer_row.disposition, PeerActionDisposition::Unknown);
    assert_eq!(peer_row.proof_ref, None, "a delivery records no proof");
    let tombstones = tombstone_count(&attempt, &store);
    assert!(tombstones > 0, "the effect committed");
    let retried =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the retry reports the pending action");
    assert_eq!(
        retried.status, "PROCESSING",
        "a delivery alone never completes the action"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstones,
        "the retry repeats no purge"
    );
    assert_eq!(action_rows(&store, &action).len(), 1, "never a second row");
}

/// A removal evidence disposition without its proof reference is not removal
/// evidence: the completion stays open and the row keeps its missing proof.
#[test]
fn a_removal_evidence_without_its_proof_keeps_the_completion_open() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = record_with_a_pending_peer(&attempt, &peer_bank);
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    let unproven = record_disposition(
        &store,
        &attempt.request_id,
        &peer_bank,
        PeerActionDisposition::RemovalEvidence,
        None,
    )
    .expect("the unproven removal is recorded");
    assert_eq!(unproven.proof_ref, None, "no proof reference is recorded");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a removal evidence without its proof cannot complete the action"
    );
    let action = erasure_action_id(&attempt.request_id);
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        load_peer_row(&store, &attempt.request_id, &peer_bank),
        unproven,
        "the row keeps the disposition it recorded, with no proof"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action],
        "the action stays pending"
    );
}

/// One peer's own durable row is the only evidence that settles it: a second
/// peer without evidence keeps the completion open however the first settled.
#[test]
fn a_second_peer_without_its_own_evidence_keeps_the_completion_open() {
    let attempt = Attempt::new();
    let settled_peer = unique("peer-bank");
    let open_peer = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record
        .peers
        .insert(settled_peer.clone(), PeerState::Acknowledged);
    record.peers.insert(open_peer.clone(), PeerState::Pending);
    record.peer_banks.insert(settled_peer.clone());
    record.peer_banks.insert(open_peer.clone());
    persist(&store, &record);
    for bank in [&attempt.bank, &settled_peer, &open_peer] {
        store
            .create_memory_item(&wiring_item(bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    begin_denial(&store, &attempt.request_id, &settled_peer).expect("the peer row begins");
    mark_acknowledged(&store, &attempt.request_id, &settled_peer, "deletion-9")
        .expect("the settled peer acknowledges");
    begin_denial(&store, &attempt.request_id, &open_peer).expect("the peer row begins");
    mark_delivered(&store, &attempt.request_id, &open_peer).expect("the delivery records");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a settled peer settles no other peer's row"
    );
    let action = erasure_action_id(&attempt.request_id);
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action],
        "the action still awaits the unsettled peer"
    );
    let peer_row = load_peer_row(&store, &attempt.request_id, &open_peer);
    assert_eq!(
        peer_row.state,
        PeerActionState::Delivered,
        "the unsettled peer keeps its delivery"
    );
    assert_eq!(peer_row.proof_ref, None, "no proof is recorded for it");
}

/// Run the durable erasure entry twice across a restart with one peer row left
/// in `prepare`'s state, then settle the peer and run the entry once more.
///
/// `prepare` records the peer-row state the restart has to survive; the body
/// pins that completion refuses on both sides of the restart, that no purge
/// repeats, and that the peer's own acknowledgement settles the same action
/// once without a second effect.
fn assert_restart_over_an_unsettled_peer(prepare: impl Fn(&SqliteStore, &str, &str)) {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let store = record_with_a_pending_peer(&attempt, &peer_bank);
    begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
    prepare(&store, &attempt.request_id, &peer_bank);
    let peer_row = load_peer_row(&store, &attempt.request_id, &peer_bank);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let live = live_set(&attempt, &store);
    let action = erasure_action_id(&attempt.request_id);
    let first = erase_confirmed_request(
        &store,
        &store,
        &store,
        &attempt.request_id,
        &attempt.args(&store, &ctx, &sink),
        &live,
    )
    .expect("the effect completes");
    assert_eq!(
        first.status, "PROCESSING",
        "an unsettled peer refuses completion"
    );
    let tombstones = tombstone_count(&attempt, &store);
    assert!(tombstones > 0, "the purge committed once");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action.clone()],
        "the action stays pending"
    );
    drop(store);
    // The restart boundary: reopen the same file and re-run the entry.
    let store = open_sqlite(&attempt.store_path());
    assert_eq!(
        load_peer_row(&store, &attempt.request_id, &peer_bank),
        peer_row,
        "the peer row restarts verbatim"
    );
    let restarted = erase_confirmed_request(
        &store,
        &store,
        &store,
        &attempt.request_id,
        &attempt.args(&store, &ctx, &sink),
        &live,
    )
    .expect("the restarted attempt reports the pending action");
    assert_eq!(
        restarted.status, "PROCESSING",
        "the restart still refuses completion"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstones,
        "the restart repeats no purge"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action.clone()],
        "the action still awaits the peer"
    );
    // The peer's own durable evidence settles the same action, once.
    mark_acknowledged(&store, &attempt.request_id, &peer_bank, "deletion-9")
        .expect("the peer acknowledges");
    let settled = erase_confirmed_request(
        &store,
        &store,
        &store,
        &attempt.request_id,
        &attempt.args(&store, &ctx, &sink),
        &live,
    )
    .expect("the settled peer lets the retry complete");
    assert_eq!(settled.status, "COMPLETED");
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstones,
        "the settle repeats no destructive work"
    );
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one allowed outcome, never a second");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
}

/// A restart over a pending peer row keeps completion open, and the peer's own
/// acknowledgement settles the same action once.
#[test]
fn a_restart_over_a_pending_peer_row_keeps_completion_open() {
    assert_restart_over_an_unsettled_peer(|_, _, _| {});
}

/// A restart over a delivered peer row keeps completion open, and the peer's
/// own acknowledgement settles the same action once.
#[test]
fn a_restart_over_a_delivered_peer_row_keeps_completion_open() {
    assert_restart_over_an_unsettled_peer(|store, request_id, peer_id| {
        mark_delivered(store, request_id, peer_id).expect("the delivery records");
    });
}

/// A reopened store over a pending peer-denial action: the durable record and
/// the peer-denial intent are persisted, and the store is closed and reopened
/// so the caller works across the restart boundary. The peer id and its
/// derived action id come back with the store.
fn reopened_peer_action() -> (Attempt, SqliteStore, String, String) {
    let attempt = Attempt::new();
    let peer_id = unique("peer-bank");
    let store = open_sqlite(&attempt.store_path());
    persist(&store, &started_record(&attempt.request_id, &attempt.scope));
    record_peer_denial_intent(
        &store,
        &attempt.request_id,
        &peer_id,
        Actor::Harness,
        super::NOW,
    )
    .expect("the intent is durable before delivery");
    let action = peer_action_id(&attempt.request_id, &peer_id);
    drop(store);
    let store = open_sqlite(&attempt.store_path());
    (attempt, store, peer_id, action)
}

/// Reconcile over the reopened store and pin that the pass claims no result.
fn assert_reconcile_keeps_pending(attempt: &Attempt, store: &SqliteStore, action: &str) {
    let sink = EraseVecSink::default();
    let env = attempt.env(store, store, &sink);
    let report = reconcile_erasure_actions(store, store, &env, &|_| None).expect("reconcile");
    assert_eq!(
        report.pending,
        vec![action.to_owned()],
        "the peer action stays pending"
    );
    assert!(report.settled.is_empty(), "{report:?}");
    let rows = action_rows(store, action);
    assert_eq!(rows.len(), 1, "one event row for the peer action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action.to_owned()],
        "the action is still owed"
    );
}

/// A reopened store keeps a peer action with no durable row pending: the pass
/// claims no result it cannot see.
#[test]
fn a_reopened_store_keeps_a_peer_action_without_a_durable_row_pending() {
    let (attempt, store, _peer_id, action) = reopened_peer_action();
    assert_reconcile_keeps_pending(&attempt, &store, &action);
    assert!(
        store
            .load_for_request(&attempt.request_id)
            .expect("list")
            .is_empty(),
        "no peer row exists to settle from"
    );
}

/// A reopened store keeps a peer action whose row is only pending open.
#[test]
fn a_reopened_store_keeps_a_pending_peer_row_pending() {
    let (attempt, store, peer_id, action) = reopened_peer_action();
    begin_denial(&store, &attempt.request_id, &peer_id).expect("the peer row begins");
    assert_reconcile_keeps_pending(&attempt, &store, &action);
    let row = load_peer_row(&store, &attempt.request_id, &peer_id);
    assert_eq!(
        row.state,
        PeerActionState::Pending,
        "the row is read, never rewritten"
    );
}

/// A reopened store keeps a peer action whose row was delivered open: a
/// delivery is not a durable result.
#[test]
fn a_reopened_store_keeps_a_delivered_peer_row_pending() {
    let (attempt, store, peer_id, action) = reopened_peer_action();
    begin_denial(&store, &attempt.request_id, &peer_id).expect("the peer row begins");
    mark_delivered(&store, &attempt.request_id, &peer_id).expect("the delivery records");
    assert_reconcile_keeps_pending(&attempt, &store, &action);
    let row = load_peer_row(&store, &attempt.request_id, &peer_id);
    assert_eq!(
        row.state,
        PeerActionState::Delivered,
        "the delivered row stays as it is"
    );
    assert_eq!(row.disposition, PeerActionDisposition::Unknown);
}
