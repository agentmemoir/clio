// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request
//! action-event log: envelope validation, record idempotency and
//! immutability, restart replay, open-copy counting, and sequence and
//! actor fidelity.
//!
//! ## Owns
//! - Envelope and idempotency assertions for `ActionEventLog`
//! - Open-copy counting assertions for `ActionEventLog`
//!
//! ## Does not own
//! - The log itself (see `action_event_log.rs`)
//! - Binding, restore, and closure gate coverage (see
//!   `subject_request_events_gates_tests.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::action_event_log::{ActionEventLog, RecordOutcome};
use crate::subject_request::RequestScope;
use crate::subject_request_events::{
    ACTION_EVENT_TYPE, ACTION_EVENT_VERSION, ActionEventKind, ActionIntent, ActionOutcome,
    ActionPayload, PendingResult, ProtectedAction, SubjectActionEvent,
};
use crate::subject_request_manifest::CopyCustodyManifest;
use clio_types::{Actor, ErrorCode};

const TIME: &str = "2026-01-01T00:00:00Z";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A subject scope for the request tests.
pub(crate) fn subject_scope() -> RequestScope {
    RequestScope::Subject {
        bank_id: "bank-a".into(),
        subject_id: "subject-1".into(),
    }
}

/// A valid intent event; tests mutate it to build other cases.
pub(crate) fn event(
    event_id: &str,
    producer: ProtectedAction,
    request_id: &str,
) -> SubjectActionEvent {
    SubjectActionEvent {
        event_type: ACTION_EVENT_TYPE.into(),
        event_id: event_id.into(),
        time: TIME.into(),
        version: ACTION_EVENT_VERSION,
        correlation_id: request_id.into(),
        sequence: 1,
        actor: Actor::System,
        payload: ActionPayload {
            producer,
            kind: ActionEventKind::Intent,
            intent: ActionIntent {
                action_id: format!("action-{event_id}"),
                scope: subject_scope(),
            },
            outcome: None,
            pending: None,
        },
    }
}

/// A terminal event for an action: its outcome, with any result still owed.
/// The action id comes from the event id's part before the last `-`.
pub(crate) fn terminal(
    event_id: &str,
    producer: ProtectedAction,
    request_id: &str,
    outcome: ActionOutcome,
    pending: Option<PendingResult>,
) -> SubjectActionEvent {
    let base = event_id.rsplit_once('-').map_or(event_id, |(base, _)| base);
    let mut recorded = event(base, producer, request_id);
    recorded.event_id = event_id.into();
    recorded.payload.kind = ActionEventKind::Outcome;
    recorded.payload.outcome = Some(outcome);
    recorded.payload.pending = pending;
    recorded
}

#[test]
fn producer_ids_are_stable_strings() {
    assert_eq!(ProtectedAction::CopyExport.producer_id(), "copy_export");
    assert_eq!(ProtectedAction::Erasure.producer_id(), "erasure");
    assert_eq!(ProtectedAction::Restore.producer_id(), "restore");
    assert_eq!(ProtectedAction::PeerDenial.producer_id(), "peer_denial");
}

/// Every durable pending result keeps the spelling a stored event carries, so
/// a new obligation can never be read back as another one.
#[test]
fn the_durable_pending_results_keep_their_stored_spellings() {
    let pinned = [
        (PendingResult::CopyManifestPending, "copy_manifest_pending"),
        (PendingResult::OutcomePending, "outcome_pending"),
        (PendingResult::PeerDenialPending, "peer_denial_pending"),
    ];
    for (pending, spelling) in pinned {
        let encoded = serde_json::to_string(&pending).expect("encode the pending result");
        assert_eq!(encoded, format!("\"{spelling}\""), "{pending:?}");
    }
}

/// A malformed-envelope case: a mutation of a valid event and the refused code.
type MalformedCase = (fn(&mut SubjectActionEvent), ErrorCode);

#[test]
fn record_refuses_a_malformed_envelope_and_stores_nothing() {
    let cases: Vec<MalformedCase> = vec![
        (
            |e| e.event_type = "other".into(),
            ErrorCode::InvalidArgument,
        ),
        (|e| e.version = 99, ErrorCode::InvalidArgument),
        (
            |e| e.time = "2026-01-01 00:00:00Z".into(),
            ErrorCode::InvalidArgument,
        ),
        (|e| e.event_id = " ".into(), ErrorCode::InvalidArgument),
        (
            |e| e.correlation_id = String::new(),
            ErrorCode::InvalidArgument,
        ),
        (
            |e| e.payload.intent.action_id = "  ".into(),
            ErrorCode::InvalidArgument,
        ),
        (
            |e| {
                e.payload.intent.scope = RequestScope::Bank {
                    bank_id: "  ".into(),
                }
            },
            ErrorCode::MissingBank,
        ),
        (
            |e| e.payload.outcome = Some(ActionOutcome::Done),
            ErrorCode::InvalidArgument,
        ),
        (
            |e| e.payload.kind = ActionEventKind::Outcome,
            ErrorCode::InvalidArgument,
        ),
    ];
    for (mutate, code) in cases {
        let mut log = ActionEventLog::new();
        let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
        mutate(&mut bad);
        assert_eq!(log.record(bad).unwrap_err().code, code);
        assert_eq!(log.events(), [] as [SubjectActionEvent; 0]);
    }
}

#[test]
fn the_same_event_id_with_the_same_content_replays_and_applies_nothing() {
    let mut log = ActionEventLog::new();
    assert_eq!(
        log.record(event("e1", ProtectedAction::CopyExport, "r1"))
            .unwrap(),
        RecordOutcome::Recorded
    );
    assert_eq!(
        log.record(event("e1", ProtectedAction::CopyExport, "r1"))
            .unwrap(),
        RecordOutcome::Replayed
    );
    assert_eq!(log.events().len(), 1);
}

#[test]
fn the_same_event_id_with_changed_content_is_refused_as_immutable() {
    let mut log = ActionEventLog::new();
    log.record(event("e1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    let changed = terminal(
        "e1",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        None,
    );
    let err = log.record(changed).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(log.events().len(), 1);
}

#[test]
fn reloaded_events_replay_after_a_modeled_restart() {
    let mut log = ActionEventLog::new();
    log.record(event("e1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    let bytes = serde_json::to_string(&log.events()).expect("serialize events");
    let reloaded: Vec<SubjectActionEvent> =
        serde_json::from_str(&bytes).expect("deserialize events");
    let mut restarted = ActionEventLog::load(reloaded);
    assert_eq!(
        restarted
            .record(event("e1", ProtectedAction::CopyExport, "r1"))
            .unwrap(),
        RecordOutcome::Replayed
    );
}

#[test]
fn open_copy_actions_counts_only_open_copy_actions_of_the_request() {
    let mut log = ActionEventLog::new();
    log.record(event("c1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    log.record(event("c2", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    log.record(event("x1", ProtectedAction::Erasure, "r1"))
        .unwrap();
    log.record(event("c3", ProtectedAction::CopyExport, "r2"))
        .unwrap();
    assert_eq!(log.open_copy_actions("r1"), 2);

    // The latest event per action wins: c1 finished, c2 still owes its
    // manifest.
    let done = terminal(
        "c1-done",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        None,
    );
    log.record(done).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);
}

#[test]
fn an_open_copy_action_is_one_that_still_owes_a_durable_result() {
    let mut log = ActionEventLog::new();
    log.record(event("c1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);

    let pending = terminal(
        "c1-pending",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        Some(PendingResult::CopyManifestPending),
    );
    log.record(pending).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);

    let closed = terminal(
        "c1-closed",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        None,
    );
    log.record(closed).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 0);
}

#[test]
fn a_pending_copy_manifest_is_discharged_by_a_terminal_event() {
    let mut log = ActionEventLog::new();
    let mut intent = event("c1", ProtectedAction::CopyExport, "r1");
    intent.payload.pending = Some(PendingResult::CopyManifestPending);
    log.record(intent).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);
    // The manifest write is the contract-level act that discharges the copy
    // obligation; the terminal event then closes the copy.
    CopyCustodyManifest::write("producer-a", "receiver-b", DIGEST, TIME, TIME)
        .expect("manifest writes");
    let owed = terminal(
        "c1-owed",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        Some(PendingResult::OutcomePending),
    );
    log.record(owed).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);
    let closed = terminal(
        "c1-closed",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        None,
    );
    log.record(closed).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 0);
}

#[test]
fn a_reloaded_log_with_a_duplicate_event_id_keeps_both_entries() {
    let mut log = ActionEventLog::new();
    log.record(event("c1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    let mut entries = log.events();
    let mut second = event("c2", ProtectedAction::CopyExport, "r1");
    second.event_id = "c1".into();
    entries.push(second);
    let reloaded = ActionEventLog::load(entries);
    assert_eq!(reloaded.events().len(), 2);
    assert_eq!(reloaded.open_copy_actions("r1"), 2);
}

fn refused_outcome() -> ActionOutcome {
    ActionOutcome::Refused { reason: "r".into() }
}

fn failed_outcome() -> ActionOutcome {
    ActionOutcome::Failed { reason: "e".into() }
}

#[test]
fn only_a_done_copy_with_nothing_owed_counts_as_closed() {
    let rows: Vec<(ActionOutcome, Option<PendingResult>, usize)> = vec![
        (ActionOutcome::Done, None, 0),
        (
            ActionOutcome::Done,
            Some(PendingResult::CopyManifestPending),
            1,
        ),
        (refused_outcome(), None, 1),
        (refused_outcome(), Some(PendingResult::OutcomePending), 1),
        (failed_outcome(), None, 1),
        (
            failed_outcome(),
            Some(PendingResult::CopyManifestPending),
            1,
        ),
    ];
    for (index, (outcome, pending, open)) in rows.into_iter().enumerate() {
        let mut log = ActionEventLog::new();
        let recorded = terminal(
            &format!("c1-{index}"),
            ProtectedAction::CopyExport,
            "r1",
            outcome,
            pending,
        );
        log.record(recorded).unwrap();
        assert_eq!(log.open_copy_actions("r1"), open);
    }
}

#[test]
fn the_writer_assigned_sequence_and_actor_are_kept_verbatim() {
    let mut log = ActionEventLog::new();
    let mut first = event("e1", ProtectedAction::CopyExport, "r1");
    first.sequence = 7;
    first.actor = Actor::Harness;
    assert_eq!(log.record(first.clone()).unwrap(), RecordOutcome::Recorded);
    assert_eq!(log.record(first).unwrap(), RecordOutcome::Replayed);
    let mut changed = event("e1", ProtectedAction::CopyExport, "r1");
    changed.sequence = 8;
    assert_eq!(log.record(changed).unwrap_err().code, ErrorCode::Forbidden);
    let stored = &log.events()[0];
    assert_eq!(stored.sequence, 7);
    assert_eq!(stored.actor, Actor::Harness);
}
