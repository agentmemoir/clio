// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the closure-fact coverage of the durable
//! peer rows: the fold that carries each frozen peer's recorded evidence into
//! the facts the request outcome is computed from, where an unevidenced peer
//! leaves an open removal action, a partial outcome, and a refused closure.
//!
//! ## Owns
//! - The open-action, partial-outcome, and refused-closure assertions over the
//!   peer-row fold
//! - The evidenced-set assertion over the same fold
//!
//! ## Does not own
//! - The fold itself and the stored-row mapping
//!   (`subject_request_store_wiring.rs`), the disposition vocabulary and its
//!   fold rules (`copy_disposition.rs`), and the peer evidence rule
//!   (`subject_peer.rs`)
//! - The completion-gate settle assertions
//!   (`erasure_retry_peer_evidence_tests.rs`,
//!   `erasure_retry_peer_pending_tests.rs`) and the shared fixtures
//!   (`subject_request_store_wiring_tests.rs`, `erasure_retry_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a retention, an unknown disposition, or an unproven
//! removal stops leaving an open removal action, or when an evidenced peer set
//! stops folding to a full closure.
//!
//! Test-only. Never compiled into production builds.

use super::erasure_retry_tests::Attempt;
use super::peer_row_closure_facts;
use super::subject_request_store_wiring_tests::{open_sqlite, persist, started_record};
use crate::copy_disposition::CopyDispositionOutcome;
use crate::subject_peer::{begin_denial, mark_acknowledged, record_disposition};
use crate::subject_request::{
    FinalOutcome, PathOwner, PathSlot, PathState, PeerState, SubjectRequestRecord,
};
use crate::subject_request_events::{ClosureBlock, accept_closure};
use crate::test_unique::unique;
use clio_store::{PeerActionDisposition, PeerActionStore};

/// One stored record with a known product path and the given peer slots.
fn record_with_peers(attempt: &Attempt, peers: &[&str]) -> SubjectRequestRecord {
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    for peer in peers {
        record.peers.insert((*peer).to_owned(), PeerState::Pending);
    }
    record.paths.insert(
        "product-path".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    record
}

/// The durable peer rows fold into the closure facts the request outcome is
/// computed from: an unproven removal, a retention, and an undispositioned row
/// each keep a removal action open, make the outcome partial, and refuse the
/// closure.
#[test]
fn an_unevidenced_peer_set_folds_to_a_partial_outcome_and_a_refused_closure() {
    let attempt = Attempt::new();
    let removed = unique("peer-removed");
    let retained = unique("peer-retained");
    let undecided = unique("peer-undecided");
    let store = open_sqlite(&attempt.store_path());
    let record = record_with_peers(&attempt, &[&removed, &retained, &undecided]);
    persist(&store, &record);
    let request_id = attempt.request_id.as_str();
    for peer in [&removed, &retained, &undecided] {
        begin_denial(&store, request_id, peer).expect("the peer row begins");
    }
    let unproven = record_disposition(
        &store,
        request_id,
        &removed,
        PeerActionDisposition::RemovalEvidence,
        None,
    )
    .expect("the unproven removal records");
    assert_eq!(unproven.proof_ref, None, "no proof reference is recorded");
    record_disposition(
        &store,
        request_id,
        &retained,
        PeerActionDisposition::RetentionApproved,
        Some("hold-7"),
    )
    .expect("the approved retention records");
    let rows = store.load_for_request(request_id).expect("list");
    assert_eq!(rows.len(), 3, "one row per frozen peer");
    let facts = peer_row_closure_facts(&record, &rows, "product-path");
    assert_eq!(
        facts.inputs.open_copy_actions, 3,
        "no peer carries the evidence that closes its removal action"
    );
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial, "never full");
    assert_eq!(
        accept_closure(&facts.inputs),
        Err(ClosureBlock::OpenCopyAction),
        "the closure gate refuses the completion"
    );
}

/// A record whose every peer row carries the evidence its removal needs folds
/// to a full closure the gate accepts.
#[test]
fn an_evidenced_peer_set_folds_to_a_full_closure() {
    let attempt = Attempt::new();
    let removed = unique("peer-removed");
    let acknowledged = unique("peer-acknowledged");
    let store = open_sqlite(&attempt.store_path());
    let record = record_with_peers(&attempt, &[&removed, &acknowledged]);
    persist(&store, &record);
    let request_id = attempt.request_id.as_str();
    begin_denial(&store, request_id, &removed).expect("the peer row begins");
    record_disposition(
        &store,
        request_id,
        &removed,
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("the removal evidence records");
    begin_denial(&store, request_id, &acknowledged).expect("the peer row begins");
    mark_acknowledged(&store, request_id, &acknowledged, "generation=1")
        .expect("the peer acknowledges");
    let rows = store.load_for_request(request_id).expect("list");
    let facts = peer_row_closure_facts(&record, &rows, "product-path");
    assert_eq!(
        facts.inputs.open_copy_actions, 0,
        "every peer carries evidence"
    );
    assert_eq!(facts.outcome, CopyDispositionOutcome::Full);
    assert_eq!(
        accept_closure(&facts.inputs),
        Ok(FinalOutcome::Completed),
        "the closure gate accepts the evidenced set"
    );
}

/// The fold reads the rows a restart returns, not the record's own peer slots:
/// a record saved with every slot disposed still folds open while the durable
/// rows carry no evidence.
#[test]
fn the_fold_reads_the_durable_rows_not_the_records_peer_slots() {
    let attempt = Attempt::new();
    let peer = unique("peer-undecided");
    let store = open_sqlite(&attempt.store_path());
    let mut record = record_with_peers(&attempt, &[&peer]);
    record.peers.insert(peer.clone(), PeerState::Disposed);
    persist(&store, &record);
    begin_denial(&store, &attempt.request_id, &peer).expect("the peer row begins");
    let rows = store.load_for_request(&attempt.request_id).expect("list");
    let facts = peer_row_closure_facts(&record, &rows, "product-path");
    assert_eq!(
        facts.inputs.open_copy_actions, 1,
        "the record's own slot is not evidence"
    );
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
    assert_eq!(
        accept_closure(&facts.inputs),
        Err(ClosureBlock::OpenCopyAction)
    );
}

/// A frozen peer slot with no durable row folds to an open removal action: a
/// missing row is never evidence of completion.
#[test]
fn a_frozen_peer_with_no_row_folds_to_an_open_action() {
    let attempt = Attempt::new();
    let peer = unique("peer-missing");
    let store = open_sqlite(&attempt.store_path());
    let record = record_with_peers(&attempt, &[&peer]);
    persist(&store, &record);
    let rows = store.load_for_request(&attempt.request_id).expect("list");
    assert!(rows.is_empty(), "no durable row exists for the frozen peer");
    let facts = peer_row_closure_facts(&record, &rows, "product-path");
    assert_eq!(
        facts.inputs.open_copy_actions, 1,
        "a missing peer row leaves an open removal action"
    );
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
    assert_eq!(
        accept_closure(&facts.inputs),
        Err(ClosureBlock::OpenCopyAction)
    );
}
