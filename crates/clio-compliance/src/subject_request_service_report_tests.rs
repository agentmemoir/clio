// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the request facade's status fold:
//! each request state's separated technical-completion and outcome fields, the
//! pending operator actions from the record and its copy rows, the derived peer
//! slots, and the `snake_case` field names.
//!
//! ## Owns
//! - The status-fold assertions over `report_of`
//! - The open-copy-row assertion the fold needs
//!
//! ## Does not own
//! - The facade under test (`subject_request_service.rs`,
//!   `subject_request_report.rs`)
//! - The shared store, record, and copy-row fixtures
//!   (`subject_request_service_fixtures.rs`)
//! - The retry assertions (`subject_request_service_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the report's field mapping, its pending-action fold, or
//! its peer fold changes.
//!
//! Test-only. Never compiled into production builds.

use crate::erasure_retry::erasure_action_id;
use crate::subject_peer::{record_disposition, snapshot_peers};
use crate::subject_request::{
    FinalOutcome, PeerEnablement, PeerState, RequestAction, RequestActionKind, RequestActionState,
    RequestKind, RequestState, SubjectRequestLedger,
};
use crate::subject_request_events::ProtectedAction;
use crate::subject_request_report::{
    PendingOperatorAction, RequestOutcome, TechnicalCompletion, report_of,
};
use crate::subject_request_service_fixtures::{NOW, copy_row, env, record, scope, store};
use crate::subject_request_store_wiring::record_to_row;
use clio_store::{CopyInventoryStore, LocalEventIntent, PeerActionDisposition};

/// The status fold maps each request state onto both separated fields.
#[test]
fn report_of_maps_each_state_to_completion_and_outcome() {
    let store = store();
    let env = env(&store);
    let cases = [
        (
            RequestState::Received,
            TechnicalCompletion::Pending,
            RequestOutcome::Open,
        ),
        (
            RequestState::InProgress,
            TechnicalCompletion::Pending,
            RequestOutcome::Open,
        ),
        (
            RequestState::TechnicalComplete,
            TechnicalCompletion::Complete,
            RequestOutcome::Open,
        ),
        (
            RequestState::AwaitingOperator,
            TechnicalCompletion::Complete,
            RequestOutcome::Open,
        ),
        (
            RequestState::Blocked {
                reason: "blocked".to_owned(),
            },
            TechnicalCompletion::Unknown,
            RequestOutcome::Blocked,
        ),
        (
            RequestState::Final(FinalOutcome::Completed),
            TechnicalCompletion::Complete,
            RequestOutcome::Completed,
        ),
        (
            RequestState::Final(FinalOutcome::PartiallyCompleted),
            TechnicalCompletion::Complete,
            RequestOutcome::PartiallyCompleted,
        ),
        (
            RequestState::Refused {
                rationale: "denied".to_owned(),
            },
            TechnicalCompletion::Complete,
            RequestOutcome::Refused,
        ),
    ];
    for (state, completion, outcome) in cases {
        let label = format!("{state:?}");
        let report = report_of(&env, &record("req-map", state)).expect("report");
        assert_eq!(
            report.technical_completion, completion,
            "technical completion for {label}"
        );
        assert_eq!(report.request_outcome, outcome, "outcome for {label}");
    }
}

/// The status fold copies the request identity and the null fields through.
#[test]
fn report_of_carries_the_record_identity() {
    let store = store();
    let env = env(&store);
    let mut source = record("req-identity", RequestState::InProgress);
    source.verification = None;
    source.deadline = Some("2026-02-01T00:00:00Z".to_owned());
    source.exception = Some("retention approved".to_owned());
    let report = report_of(&env, &source).expect("report");
    assert_eq!(report.request_id, "req-identity");
    assert_eq!(report.kind, RequestKind::Erasure);
    assert_eq!(report.scope, scope());
    assert_eq!(report.state, RequestState::InProgress);
    assert_eq!(report.deadline.as_deref(), Some("2026-02-01T00:00:00Z"));
    assert_eq!(report.exception.as_deref(), Some("retention approved"));
    assert_eq!(report.peer_enablement, PeerEnablement::NotApplicable);
    assert!(report.peers.is_empty());
}

/// Only open actions of the record are pending, in action-id order.
#[test]
fn report_of_lists_open_record_actions_only() {
    let store = store();
    let env = env(&store);
    let mut source = record("req-actions", RequestState::InProgress);
    source.actions.insert(
        "act-open".to_owned(),
        RequestAction {
            kind: RequestActionKind::Copy,
            state: RequestActionState::Open,
        },
    );
    source.actions.insert(
        "act-closed".to_owned(),
        RequestAction {
            kind: RequestActionKind::Notice,
            state: RequestActionState::Closed,
        },
    );
    let report = report_of(&env, &source).expect("report");
    assert_eq!(
        report.pending_operator_actions,
        vec![PendingOperatorAction {
            action_id: "act-open".to_owned(),
            kind: Some(RequestActionKind::Copy),
        }]
    );
}

/// An open stored copy row is a pending action; a done row is not.
#[test]
fn report_of_lists_open_copy_rows_only() {
    let store = store();
    let env = env(&store);
    CopyInventoryStore::save(
        &store,
        &copy_row("copy-open", "req-copy", "pending", Some("gap")),
    )
    .expect("save the open copy");
    CopyInventoryStore::save(&store, &copy_row("copy-done", "req-copy", "done", None))
        .expect("save the done copy");
    let report = report_of(&env, &record("req-copy", RequestState::InProgress)).expect("report");
    assert_eq!(
        report.pending_operator_actions,
        vec![PendingOperatorAction {
            action_id: "copy-open".to_owned(),
            kind: Some(RequestActionKind::Copy),
        }]
    );
}

/// A durable pending local event bound to the request is a pending action; one
/// bound to another request is never listed.
#[test]
fn report_of_lists_pending_local_event_actions_of_this_request_only() {
    let store = store();
    let env = env(&store);
    let request_id = "req-events";
    let mine = erasure_action_id(request_id);
    let theirs = erasure_action_id("req-other");
    for action_id in [&mine, &theirs] {
        env.events
            .record_intent(&LocalEventIntent {
                action_id: action_id.as_str(),
                actor: "harness",
                scope: ProtectedAction::Erasure.producer_id(),
                kind: ProtectedAction::Erasure.producer_id(),
                event_time: NOW,
            })
            .expect("record the pending intent");
    }
    let report = report_of(&env, &record(request_id, RequestState::InProgress)).expect("report");
    assert_eq!(
        report.pending_operator_actions,
        vec![PendingOperatorAction {
            action_id: mine,
            kind: None,
        }]
    );
    assert!(
        !report
            .pending_operator_actions
            .iter()
            .any(|action| action.action_id == theirs),
        "another request's pending action is never listed"
    );
}

/// The peer slots are derived from the durable rows, not the frozen snapshot.
#[test]
fn report_of_derives_peer_slots_from_durable_rows() {
    let store = store();
    let env = env(&store);
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register_with_enablement(
            "req-peer",
            RequestKind::Erasure,
            &scope(),
            PeerEnablement::Required,
            &["peer-1"],
            &[],
        )
        .expect("register the peer request");
    let source = ledger.record("req-peer").cloned().expect("peer record");
    env.durable
        .save(&record_to_row(&source))
        .expect("persist the peer request");
    snapshot_peers(env.peers, &source).expect("snapshot the peer");
    record_disposition(
        env.peers,
        "req-peer",
        "peer-1",
        PeerActionDisposition::RemovalEvidence,
        Some("proof"),
    )
    .expect("record the peer disposition");
    let report = report_of(&env, &source).expect("report");
    assert_eq!(report.peers.get("peer-1"), Some(&PeerState::Disposed));
}

/// The serialized report carries the three required `snake_case` field names.
#[test]
fn report_serializes_the_separated_field_names() {
    let store = store();
    let env = env(&store);
    let report = report_of(&env, &record("req-json", RequestState::InProgress)).expect("report");
    let value = serde_json::to_value(&report).expect("serialize the report");
    for key in [
        "technical_completion",
        "pending_operator_actions",
        "request_outcome",
    ] {
        assert!(
            value.get(key).is_some(),
            "the report must carry the field {key}"
        );
    }
}
