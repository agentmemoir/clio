// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request
//! register binding: id, kind, scope, and the frozen peer snapshot.
//!
//! ## Owns
//! - Register open, replay, kind, and scope-refusal assertions
//! - Peer-snapshot freeze and validation assertions
//!
//! ## Does not own
//! - Transition and apply assertions (`subject_request_tests.rs`)
//! - The durable record fields (`subject_request_frozen_tests.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::{
    PeerState, RegisterOutcome, RequestAction, RequestActionKind, RequestActionState,
    RequestCommand, RequestKind, RequestScope, RequestState, SubjectRequestLedger,
    SubjectRequestRecord,
};
use clio_types::ErrorCode;
use std::collections::BTreeMap;

fn subject_scope() -> RequestScope {
    RequestScope::Subject {
        bank_id: "bank-a".into(),
        subject_id: "subject-1".into(),
    }
}

fn bank_scope() -> RequestScope {
    RequestScope::Bank {
        bank_id: "bank-a".into(),
    }
}

fn subject_in(bank_id: &str, subject_id: &str) -> RequestScope {
    RequestScope::Subject {
        bank_id: bank_id.into(),
        subject_id: subject_id.into(),
    }
}

#[test]
fn register_opens_a_received_record_with_the_given_kind_scope_and_peers() {
    let mut ledger = SubjectRequestLedger::new();
    let outcome = ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &subject_scope(),
            &["peer-b", "peer-a", "peer-b"],
        )
        .unwrap();
    assert_eq!(outcome, RegisterOutcome::Opened);
    let record = ledger.record("r1").expect("record stored");
    assert_eq!(record.kind, RequestKind::Erasure);
    assert_eq!(record.scope, subject_scope());
    assert_eq!(record.state, RequestState::Received);
    let peers: BTreeMap<String, PeerState> = [
        ("peer-a".to_owned(), PeerState::Pending),
        ("peer-b".to_owned(), PeerState::Pending),
    ]
    .into_iter()
    .collect();
    assert_eq!(record.peers, peers, "duplicates collapse and slots sort");
}

#[test]
fn same_id_same_kind_and_scope_replays_the_stored_result() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    ledger
        .apply("r1", &RequestCommand::MarkTechnicalComplete)
        .unwrap();
    let before = ledger.record("r1").cloned();
    let outcome = ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    assert_eq!(
        outcome,
        RegisterOutcome::Replayed {
            state: RequestState::TechnicalComplete
        },
        "a replay returns the exact stored state"
    );
    assert_eq!(
        ledger.record("r1").cloned(),
        before,
        "a replay applies nothing"
    );
}

#[test]
fn same_id_changed_kind_is_refused_without_state_change() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Copy, &subject_scope(), &[])
        .unwrap();
    let before = ledger.record("r1").cloned();
    let err = ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        ledger.record("r1").cloned(),
        before,
        "a refusal changes nothing"
    );
}

#[test]
fn same_id_changed_scope_is_refused_without_state_change() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &bank_scope(), &[])
        .unwrap();
    let before = ledger.record("r1").cloned();
    let err = ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        ledger.record("r1").cloned(),
        before,
        "a refusal changes nothing"
    );
}

#[test]
fn subjects_and_banks_are_isolated_and_changed_scopes_are_refused() {
    let others = [
        subject_in("bank-a", "subject-2"),
        subject_in("bank-b", "subject-1"),
    ];
    for other in others {
        let mut ledger = SubjectRequestLedger::new();
        ledger
            .register("r1", RequestKind::Erasure, &subject_scope(), &[])
            .unwrap();
        ledger
            .register("r2", RequestKind::Erasure, &other, &[])
            .unwrap();
        let before = ledger.record("r1").cloned();
        ledger
            .register("r1", RequestKind::Erasure, &other, &[])
            .unwrap_err();
        assert_eq!(
            ledger.record("r1").cloned(),
            before,
            "a refusal changes nothing: {other:?}"
        );
    }
}

#[test]
fn a_replay_ignores_the_incoming_peer_list_entirely() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &["peer-1"])
        .unwrap();
    let outcome = ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &subject_scope(),
            &[" ", "peer-2"],
        )
        .unwrap();
    assert_eq!(
        outcome,
        RegisterOutcome::Replayed {
            state: RequestState::Received
        },
        "a replay keeps the stored snapshot, even for a blank incoming id"
    );
    let record = ledger.record("r1").expect("record stored");
    assert_eq!(
        record.peers.keys().collect::<Vec<_>>(),
        vec!["peer-1"],
        "a replay must not replace the frozen snapshot"
    );
}

#[test]
fn marking_technical_complete_with_an_open_copy_action_is_refused() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    let mut record = ledger.record("r1").cloned().expect("record stored");
    record.actions.insert(
        "copy-1".into(),
        RequestAction {
            kind: RequestActionKind::Copy,
            state: RequestActionState::Open,
        },
    );
    let mut ledger = SubjectRequestLedger::load(vec![record]);
    let err = ledger
        .apply("r1", &RequestCommand::MarkTechnicalComplete)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        ledger.record("r1").map(|r| r.state.clone()),
        Some(RequestState::InProgress),
        "a refusal changes nothing"
    );
}

#[test]
fn reloaded_records_replay_after_a_modeled_restart() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &subject_scope(),
            &["peer-1", "peer-2"],
        )
        .unwrap();
    let bytes = serde_json::to_string(&ledger.records()).expect("serialize records");
    let reloaded: Vec<SubjectRequestRecord> =
        serde_json::from_str(&bytes).expect("deserialize records");
    let mut restarted = SubjectRequestLedger::load(reloaded);
    let outcome = restarted
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    assert!(matches!(outcome, RegisterOutcome::Replayed { .. }));
    let record = restarted.record("r1").expect("record reloaded");
    assert_eq!(
        record.peers.keys().collect::<Vec<_>>(),
        vec!["peer-1", "peer-2"],
        "the snapshot survives the restart"
    );
    let other = subject_in("bank-a", "subject-2");
    let err = restarted
        .register("r1", RequestKind::Erasure, &other, &[])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn a_blank_peer_id_is_refused_and_nothing_is_stored() {
    let mut ledger = SubjectRequestLedger::new();
    let err = ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &subject_scope(),
            &["peer-1", " "],
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(ledger.record("r1").is_none());
}

#[test]
fn register_validates_the_id_and_the_scope() {
    let mut ledger = SubjectRequestLedger::new();
    let err = ledger
        .register("  ", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &RequestScope::Bank {
                bank_id: String::new(),
            },
            &[],
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
    let err = ledger
        .register(
            "r1",
            RequestKind::Erasure,
            &RequestScope::Subject {
                bank_id: "bank-a".into(),
                subject_id: "   ".into(),
            },
            &[],
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(ledger.record("r1").is_none());
}
