// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the disposition-evidence coverage of one
//! peer row: an evidenced operator disposition resolves a refused row, an
//! approved retention is recorded but leaves the peer open, a second
//! disposition with different evidence refuses, and a removal evidence without
//! its proof reference never reports a disposal.
//!
//! ## Owns
//! - The refused-row disposition, retention, and missing-proof assertions
//!
//! ## Does not own
//! - The lifecycle helpers and the slot fold (`subject_peer.rs`), the row
//!   contract (`clio-store`'s `peer_actions`), the scratch SQLite fixture
//!   (`subject_peer_fixtures.rs`), and the shared record fixture
//!   (`subject_peer_tests.rs`)
//! - The completion gate over the folded slots (`erasure_retry.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a refused row stops taking the operator's disposition,
//! when an approval starts closing a peer, or when a removal evidence without
//! its proof starts reporting a disposal.
//!
//! Test-only. Never compiled into production builds.

use super::subject_peer_fixtures::{Scratch, open_sqlite};
use super::subject_peer_tests::open_record;
use crate::subject_peer::{derived_peer_states, mark_refused, record_disposition, snapshot_peers};
use crate::subject_request::{PeerEnablement, PeerState};
use clio_store::{PeerActionDisposition, PeerActionState, PeerActionStore};
use clio_types::ErrorCode;

/// An evidenced operator disposition resolves a refused peer row without
/// losing the refusal: the row keeps its state and records the disposition.
#[test]
fn an_evidenced_disposition_resolves_a_refused_peer_row() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    let disposed = {
        let store = open_sqlite(&path);
        snapshot_peers(&store, &record).expect("the snapshot");
        mark_refused(&store, "req-1", "peer-1").expect("the refusal records");
        let disposed = record_disposition(
            &store,
            "req-1",
            "peer-1",
            PeerActionDisposition::RemovalEvidence,
            Some("deletion-9"),
        )
        .expect("a refused row takes the operator's evidenced disposition");
        assert_eq!(
            disposed.state,
            PeerActionState::Refused,
            "the row keeps its refusal state"
        );
        assert_eq!(disposed.disposition, PeerActionDisposition::RemovalEvidence);
        assert_eq!(disposed.proof_ref.as_deref(), Some("deletion-9"));
        assert_eq!(
            derived_peer_states(&record, &store.load_for_request("req-1").expect("list"))["peer-1"],
            PeerState::Disposed,
            "the evidenced disposition resolves the slot"
        );
        disposed
    };
    let reopened = open_sqlite(&path);
    assert_eq!(
        reopened
            .load_for_request("req-1")
            .expect("the restart read"),
        vec![disposed],
        "the disposed refusal restarts verbatim"
    );
}

/// An approved retention is recorded on a refused row and leaves the peer
/// open: an approval is not removal evidence.
#[test]
fn an_approved_retention_on_a_refused_row_leaves_the_peer_open() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    snapshot_peers(&store, &record).expect("the snapshot");
    mark_refused(&store, "req-1", "peer-1").expect("the refusal records");
    let retained = record_disposition(
        &store,
        "req-1",
        "peer-1",
        PeerActionDisposition::RetentionApproved,
        Some("hold-7"),
    )
    .expect("the approved retention is recorded");
    assert_eq!(retained.state, PeerActionState::Refused);
    assert_eq!(
        retained.disposition,
        PeerActionDisposition::RetentionApproved
    );
    assert_eq!(
        derived_peer_states(&record, &store.load_for_request("req-1").expect("list"))["peer-1"],
        PeerState::Pending,
        "an approved retention is not a disposal"
    );
}

/// A refused row holds one disposition: different evidence refuses and the
/// recorded evidence stands.
#[test]
fn a_refused_row_refuses_a_second_disposition_with_different_evidence() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    snapshot_peers(&store, &record).expect("the snapshot");
    mark_refused(&store, "req-1", "peer-1").expect("the refusal records");
    let first = record_disposition(
        &store,
        "req-1",
        "peer-1",
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("the first disposition records");
    let err = record_disposition(
        &store,
        "req-1",
        "peer-1",
        PeerActionDisposition::RetentionApproved,
        Some("hold-7"),
    )
    .expect_err("different evidence must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        store.load_for_request("req-1").expect("list"),
        vec![first.clone()],
        "the stored evidence stands"
    );
    let duplicate = record_disposition(
        &store,
        "req-1",
        "peer-1",
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("the same evidence is tolerated");
    assert_eq!(duplicate, first, "the row is recorded once");
}

/// A removal evidence disposition without its proof reference never reports a
/// disposal, so the slot fold and the completion gate agree over one row.
#[test]
fn a_removal_evidence_disposition_without_its_proof_never_reports_disposed() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    snapshot_peers(&store, &record).expect("the snapshot");
    let unproven = record_disposition(
        &store,
        "req-1",
        "peer-1",
        PeerActionDisposition::RemovalEvidence,
        None,
    )
    .expect("the disposition is recorded");
    assert_eq!(unproven.disposition, PeerActionDisposition::RemovalEvidence);
    assert_eq!(unproven.proof_ref, None, "no proof reference is recorded");
    assert_eq!(
        derived_peer_states(&record, &store.load_for_request("req-1").expect("list"))["peer-1"],
        PeerState::Pending,
        "a removal evidence without its proof is not a disposal"
    );
}
