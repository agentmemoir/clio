// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-bank coordination refusal
//! tests: incomplete confirmation, changed bank membership, mismatched
//! confirmations, and scope stability across a retry. Test-only: never
//! compiled into production builds.
//!
//! ## Owns
//! - Behaviour assertions for `require_all_banks_confirmed` and
//!   `coordinated_erase_request`
//! - The counting store double that proves a refusal happens before any
//!   erasure effect
//!
//! ## Does not own
//! - The coordination check itself (`subject_bank_coordination.rs`)
//! - The erasure workflow tests (`erase_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the coordination refusal changes; they never
//! change it.
//!
//! Keep this module focused on the coordination refusal.

use crate::erase::{EraseArgs, EraseVecSink};
use crate::subject_bank_coordination::{coordinated_erase_request, require_all_banks_confirmed};
use crate::subject_request::{
    FinalOutcome, PeerState, RequestKind, RequestScope, RequestState, SubjectRequestRecord,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, ItemIdentity, SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The bank the confirmed request is bound to.
const BANK_A: &str = "bank-a";
/// The second bank that shares the subject key.
const BANK_B: &str = "bank-b";
/// The subject both banks hold.
const SUBJECT: &str = "subject-1";

/// A confirmed erasure record with the given kind, scope, and peer slots.
fn confirmed(
    kind: RequestKind,
    scope: RequestScope,
    peers: BTreeMap<String, PeerState>,
) -> SubjectRequestRecord {
    SubjectRequestRecord {
        request_id: unique("req"),
        kind,
        scope,
        state: RequestState::InProgress,
        verification: None,
        deadline: None,
        actions: BTreeMap::new(),
        paths: BTreeMap::new(),
        peers,
        exception: None,
    }
}

/// The two-bank scenario record: bank A's subject also lives in bank B.
fn two_bank(peer_b: PeerState) -> SubjectRequestRecord {
    let mut peers = BTreeMap::new();
    peers.insert(BANK_B.to_owned(), peer_b);
    confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: SUBJECT.into(),
        },
        peers,
    )
}

/// The banks sharing the subject key now.
fn shared_banks(banks: &[&str]) -> Vec<String> {
    banks.iter().map(|bank| (*bank).to_owned()).collect()
}

/// Test double: counts `erase_subject` calls; erasure always succeeds. The
/// `banks` preset is the live bank set the effect would fan out to (the real
/// stores read it through `erase_bank_set`); the seam reads it back from this
/// same store, so the check and the effect never diverge. The failure mode
/// mirrors the `MockEraseStore` pattern in `erase_tests.rs`.
struct CountingStore {
    erase_calls: AtomicUsize,
    banks: Vec<String>,
}

impl CountingStore {
    fn with_banks(banks: Vec<String>) -> Self {
        Self {
            erase_calls: AtomicUsize::new(0),
            banks,
        }
    }

    fn erase_calls(&self) -> usize {
        self.erase_calls.load(Ordering::SeqCst)
    }

    /// The live bank set, read from the effect's own store.
    fn effect_bank_set(&self) -> Vec<String> {
        self.banks.clone()
    }
}

impl ComplianceStore for CountingStore {
    fn list_subject_items(
        &self,
        _bank_id: &str,
        _subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError> {
        Ok(vec![])
    }

    fn erase_subject(&self, _spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
        Ok(SubjectEraseOutcome {
            item_ids: vec![],
            categories: vec![],
            item_hashes: vec![],
            subject_hash: "subject-hash".into(),
            vectors_purged: 0,
            assoc_edges_purged: 0,
            memtree_nodes_purged: 0,
            triples_purged: 0,
            derived_items_purged: 0,
            persona_context_rows_purged: 0,
            tombstones_written: 0,
        })
    }

    fn subject_dek_destroyed(&self, _subject_id: &str) -> Result<bool, AmError> {
        Ok(false)
    }

    fn read_tombstones(
        &self,
        _bank_id: &str,
        _request_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }

    fn list_tombstones(&self, _bank_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }
}

/// Erase arguments bound to `store`, `ctx`, and `sink` (the shared
/// `subject_request_store_wiring_tests.rs` argument shape, mirrored here
/// because test modules cannot cross-import private seam doubles).
fn erase_args<'a>(
    store: &'a dyn ComplianceStore,
    ctx: &'a OpContext,
    sink: &'a EraseVecSink,
    request_id: &'a str,
    subject_id: &'a str,
) -> EraseArgs<'a> {
    EraseArgs {
        store,
        maint: None,
        ctx,
        subject_id,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt: b"test-salt",
        timestamp: "2026-01-01T00:00:00Z",
        sink,
    }
}

/// One coordination refusal run: returns the error and the store's call count.
/// The live set is read from the effect's own store, never passed separately.
fn refused_erase(record: &SubjectRequestRecord, live_banks: &[String]) -> (AmError, usize) {
    let store = CountingStore::with_banks(live_banks.to_vec());
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&store, &ctx, &sink, &record.request_id, SUBJECT);
    let live = store.effect_bank_set();
    let err =
        coordinated_erase_request(&args, record, &live).expect_err("the coordination must refuse");
    (err, store.erase_calls())
}

#[test]
fn pending_peer_bank_refuses_before_effect() {
    let record = two_bank(PeerState::Pending);
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A, BANK_B]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "a pending peer must refuse before any effect");
}

#[test]
fn joined_bank_refuses_before_effect() {
    let record = two_bank(PeerState::Acknowledged);
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A, BANK_B, "bank-c"]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "a joined bank must refuse before any effect");
}

#[test]
fn departed_bank_refuses_before_effect() {
    let record = two_bank(PeerState::Acknowledged);
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "a departed bank must refuse before any effect");
}

#[test]
fn all_banks_confirmed_erasure_proceeds() {
    let record = two_bank(PeerState::Acknowledged);
    let store = CountingStore::with_banks(shared_banks(&[BANK_A, BANK_B]));
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&store, &ctx, &sink, &record.request_id, SUBJECT);
    let live = store.effect_bank_set();
    let outcome = coordinated_erase_request(&args, &record, &live)
        .expect("every coordinating bank confirmed");
    assert_eq!(outcome.status, "COMPLETED");
    assert_eq!(store.erase_calls(), 1);
}

#[test]
fn refusal_never_changes_the_confirmed_record() {
    let record = two_bank(PeerState::Pending);
    let before = record.clone();
    let _ = require_all_banks_confirmed(&record, &shared_banks(&[BANK_A, BANK_B]))
        .expect_err("the pending peer must refuse");
    assert_eq!(record, before, "a refusal must change nothing it owns");
}

#[test]
fn single_bank_scope_needs_no_peers() {
    let record = confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: SUBJECT.into(),
        },
        BTreeMap::new(),
    );
    require_all_banks_confirmed(&record, &shared_banks(&[BANK_A]))
        .expect("one bank with no peers is fully coordinated");
}

#[test]
fn bank_scope_needs_no_peers() {
    let record = confirmed(
        RequestKind::Erasure,
        RequestScope::Bank {
            bank_id: BANK_A.into(),
        },
        BTreeMap::new(),
    );
    require_all_banks_confirmed(&record, &shared_banks(&[BANK_A]))
        .expect("one bank scope with no peers is fully coordinated");
}

#[test]
fn a_record_outside_in_progress_authorizes_no_erasure() {
    let states = [
        RequestState::Received,
        RequestState::TechnicalComplete,
        RequestState::AwaitingOperator,
        RequestState::Blocked {
            reason: "hold".into(),
        },
        RequestState::Final(FinalOutcome::Completed),
        RequestState::Final(FinalOutcome::PartiallyCompleted),
        RequestState::Refused {
            rationale: "no".into(),
        },
    ];
    for state in states {
        let mut record = two_bank(PeerState::Acknowledged);
        record.state = state.clone();
        let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A, BANK_B]));
        assert_eq!(err.code, ErrorCode::Forbidden, "{state:?}");
        assert_eq!(calls, 0, "{state:?} must refuse before any effect");
    }
}

#[test]
fn foreign_request_id_refuses() {
    let mut record = two_bank(PeerState::Acknowledged);
    record.request_id = unique("other-req");
    let store = CountingStore::with_banks(shared_banks(&[BANK_A, BANK_B]));
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let mut args = erase_args(&store, &ctx, &sink, &record.request_id, SUBJECT);
    let foreign_id = unique("erasure-req");
    args.request_id = foreign_id.as_str();
    let live = store.effect_bank_set();
    let err = coordinated_erase_request(&args, &record, &live)
        .expect_err("a foreign request id must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(store.erase_calls(), 0);
}

#[test]
fn non_erasure_request_refuses() {
    let record = confirmed(
        RequestKind::Copy,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: SUBJECT.into(),
        },
        BTreeMap::new(),
    );
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "a copy request must never erase");
}

#[test]
fn scope_of_another_subject_refuses() {
    let record = confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: "subject-2".into(),
        },
        BTreeMap::new(),
    );
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "another subject's scope must never erase");
}

#[test]
fn scope_of_another_bank_refuses() {
    let record = confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: "bank-c".into(),
            subject_id: SUBJECT.into(),
        },
        BTreeMap::new(),
    );
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "another bank's scope must never erase");
}

#[test]
fn disposed_peer_is_not_authority() {
    let record = two_bank(PeerState::Disposed);
    let (err, calls) = refused_erase(&record, &shared_banks(&[BANK_A, BANK_B]));
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(calls, 0, "a disposed peer is not an erasure authority");
}
