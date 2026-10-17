// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the request facade's refusal and
//! operator-term surface: the scope-mismatch refusal on `open`, the closure
//! refusals on `close`, the privileged-actor gate on `peer_disposition`, the
//! confirmation gate on `retry_erasure`, and the recording of the operator's
//! verification decision and deadline.
//!
//! ## Owns
//! - The facade refusal assertions
//! - The operator-term recording and replay assertions
//!
//! ## Does not own
//! - The facade under test (`subject_request_service.rs`)
//! - The shared store, record, and retry fixtures
//!   (`subject_request_service_fixtures.rs`)
//! - The status-fold assertions (`subject_request_service_report_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a facade verb stops refusing a mis-scoped,
//! unprivileged, or unconfirmed call, or stops recording the operator terms.
//!
//! Keep this module focused on the facade refusal and operator-term tests.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::EraseVecSink;
use crate::subject_request::{
    PathOwner, PathSlot, PathState, PeerEnablement, PeerState, RequestAction, RequestActionKind,
    RequestActionState, RequestKind, RequestScope, RequestState, VerificationResult,
};
use crate::subject_request_service::{close, open, peer_disposition, retry_erasure};
use crate::subject_request_service_fixtures::{
    ctx, env, persist, record, retry_args, scope, store,
};
use crate::test_unique::unique;
use clio_store::{LocalEventStore, PeerActionDisposition};
use clio_types::{Actor, Bank, ErrorCode, OpContext};

/// A request opened with the operator's decision and deadline records both.
#[test]
fn open_records_the_operator_verification_and_deadline() {
    let store = store();
    let request_id = unique("req-open");
    let report = open(
        &env(&store),
        &ctx("bank-a"),
        &request_id,
        RequestKind::Erasure,
        &scope(),
        PeerEnablement::NotApplicable,
        &[],
        &[],
        Some(VerificationResult::Verified),
        Some("2026-03-01T00:00:00Z".to_owned()),
    )
    .expect("the request opens");
    assert_eq!(
        report.verification,
        Some(VerificationResult::Verified),
        "the operator decision is recorded"
    );
    assert_eq!(
        report.deadline.as_deref(),
        Some("2026-03-01T00:00:00Z"),
        "the optional deadline is recorded"
    );
}

/// A replayed open keeps its stored operator terms and ignores the new ones.
#[test]
fn a_replayed_open_ignores_the_incoming_terms() {
    let store = store();
    let request_id = unique("req-replay");
    open(
        &env(&store),
        &ctx("bank-a"),
        &request_id,
        RequestKind::Erasure,
        &scope(),
        PeerEnablement::NotApplicable,
        &[],
        &[],
        Some(VerificationResult::Verified),
        Some("2026-03-01T00:00:00Z".to_owned()),
    )
    .expect("the first open records the terms");
    let report = open(
        &env(&store),
        &ctx("bank-a"),
        &request_id,
        RequestKind::Erasure,
        &scope(),
        PeerEnablement::NotApplicable,
        &[],
        &[],
        None,
        None,
    )
    .expect("the replay succeeds");
    assert_eq!(report.verification, Some(VerificationResult::Verified));
    assert_eq!(report.deadline.as_deref(), Some("2026-03-01T00:00:00Z"));
}

/// A reused id bound to a changed scope refuses on the facade path.
#[test]
fn open_refuses_a_reused_id_with_a_changed_scope() {
    let store = store();
    let request_id = unique("req-scope");
    persist(&store, &record(&request_id, RequestState::InProgress));
    let changed = RequestScope::Bank {
        bank_id: unique("bank"),
    };
    let err = open(
        &env(&store),
        &ctx("bank-a"),
        &request_id,
        RequestKind::Erasure,
        &changed,
        PeerEnablement::NotApplicable,
        &[],
        &[],
        None,
        None,
    )
    .expect_err("a changed scope refuses");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// A product path the stored record does not know refuses closure.
#[test]
fn close_refuses_an_unknown_product_path() {
    let store = store();
    let request_id = unique("req-close-path");
    persist(&store, &record(&request_id, RequestState::InProgress));
    let err = close(&env(&store), &ctx("bank-a"), &request_id, "unknown-path")
        .expect_err("an unknown product path refuses closure");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// An open required copy action refuses closure.
#[test]
fn close_refuses_an_open_copy_action() {
    let store = store();
    let request_id = unique("req-close-copy");
    let mut rec = record(&request_id, RequestState::InProgress);
    rec.paths.insert(
        "product-path".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    rec.actions.insert(
        "copy-1".into(),
        RequestAction {
            kind: RequestActionKind::Copy,
            state: RequestActionState::Open,
        },
    );
    persist(&store, &rec);
    let err = close(&env(&store), &ctx("bank-a"), &request_id, "product-path")
        .expect_err("an open copy action refuses closure");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// A frozen peer slot with no durable row refuses closure: a missing row is an
/// open removal action, never evidence of completion.
#[test]
fn close_refuses_a_frozen_peer_with_no_durable_row() {
    let store = store();
    let request_id = unique("req-close-peer");
    let mut rec = record(&request_id, RequestState::InProgress);
    rec.paths.insert(
        "product-path".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    rec.peers.insert(unique("peer"), PeerState::Pending);
    rec.peer_enablement = PeerEnablement::Required;
    persist(&store, &rec);
    let err = close(&env(&store), &ctx("bank-a"), &request_id, "product-path")
        .expect_err("a peer without a durable row refuses closure");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// A non-privileged actor can record no peer disposition.
#[test]
fn peer_disposition_refuses_a_non_privileged_actor() {
    let store = store();
    let request_id = unique("req-peer-gate");
    persist(&store, &record(&request_id, RequestState::InProgress));
    let agent = OpContext::new(Bank::new("bank-a".to_owned()), Actor::Agent);
    let err = peer_disposition(
        &env(&store),
        &agent,
        &request_id,
        "peer-1",
        PeerActionDisposition::RemovalEvidence,
        Some("proof"),
    )
    .expect_err("a non-privileged actor refuses");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// An unconfirmed retry refuses before any durable intent is written.
#[test]
fn retry_erasure_refuses_an_unconfirmed_call_without_writing() {
    let store = store();
    let request_id = unique("req-retry-confirm");
    persist(&store, &record(&request_id, RequestState::InProgress));
    let ctx = ctx("bank-a");
    let salt = [0u8; 16];
    let sink = EraseVecSink::default();
    let journal = std::env::temp_dir().join(unique("svc-confirm-journal"));
    let mut args = retry_args(
        &store,
        &ctx,
        &sink,
        &journal,
        &request_id,
        "subject-1",
        &salt,
    );
    args.confirm = false;
    let err = retry_erasure(
        &env(&store),
        &ctx,
        &request_id,
        &args,
        &["bank-a".to_owned()],
    )
    .expect_err("an unconfirmed retry refuses");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        store.list_events().expect("events").is_empty(),
        "no durable intent is written"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "no action is left pending"
    );
}
