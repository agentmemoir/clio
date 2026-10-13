// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request
//! action-event contract: envelope validation, record idempotency and
//! immutability, open-copy counting, the public-binding gate, the restore
//! gate, and closure acceptance.
//!
//! ## Owns
//! - Envelope and idempotency assertions for `ActionEventLog`
//! - Gate and closure acceptance assertions for the pure check functions
//!
//! ## Does not own
//! - The action-event contract itself (see `subject_request_events.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::RequestScope;
use crate::subject_request_events::{
    ACTION_EVENT_TYPE, ACTION_EVENT_VERSION, ActionEventLog, ActionIntent, ActionOutcome,
    BindingControls, BindingRefusal, ClosureBlock, ClosureInputs, PendingResult, ProtectedAction,
    RestoreInputs, RestoreRefusal, SubjectActionEvent, accept_closure, check_binding,
    check_restore,
};
use crate::subject_request_manifest::CopyCustodyManifest;
use clio_types::ErrorCode;

const TIME: &str = "2026-01-01T00:00:00Z";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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

fn event(event_id: &str, producer: ProtectedAction, request_id: &str) -> SubjectActionEvent {
    SubjectActionEvent {
        event_type: ACTION_EVENT_TYPE.into(),
        event_id: event_id.into(),
        time: TIME.into(),
        version: ACTION_EVENT_VERSION,
        correlation_id: request_id.into(),
        payload: crate::subject_request_events::ActionPayload {
            producer,
            intent: ActionIntent {
                action_id: format!("action-{event_id}"),
                scope: subject_scope(),
            },
            outcome: None,
            pending: None,
        },
    }
}

#[test]
fn producer_ids_are_stable_strings() {
    assert_eq!(ProtectedAction::CopyExport.producer_id(), "copy_export");
    assert_eq!(ProtectedAction::Erasure.producer_id(), "erasure");
    assert_eq!(ProtectedAction::Restore.producer_id(), "restore");
}

#[test]
fn record_refuses_a_malformed_envelope_and_stores_nothing() {
    let mut log = ActionEventLog::new();
    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.event_type = "other".into();
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.version = 99;
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.time = "2026-01-01 00:00:00Z".into();
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.payload.intent.action_id = "  ".into();
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.payload.intent.scope = RequestScope::Bank {
        bank_id: "  ".into(),
    };
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);

    assert_eq!(log.events(), [] as [SubjectActionEvent; 0]);
}

#[test]
fn record_refuses_blank_event_and_correlation_ids() {
    let mut log = ActionEventLog::new();
    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.event_id = " ".into();
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut bad = event("e1", ProtectedAction::CopyExport, "r1");
    bad.correlation_id = String::new();
    let err = log.record(bad).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn the_same_event_id_with_the_same_content_replays_and_applies_nothing() {
    let mut log = ActionEventLog::new();
    assert_eq!(
        log.record(event("e1", ProtectedAction::CopyExport, "r1"))
            .unwrap(),
        crate::subject_request_events::RecordOutcome::Recorded
    );
    assert_eq!(
        log.record(event("e1", ProtectedAction::CopyExport, "r1"))
            .unwrap(),
        crate::subject_request_events::RecordOutcome::Replayed
    );
    assert_eq!(log.events().len(), 1);
}

#[test]
fn the_same_event_id_with_changed_content_is_refused_as_immutable() {
    let mut log = ActionEventLog::new();
    log.record(event("e1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    let mut changed = event("e1", ProtectedAction::CopyExport, "r1");
    changed.payload.outcome = Some(ActionOutcome::Done);
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
        crate::subject_request_events::RecordOutcome::Replayed
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
    let mut done = event("c1", ProtectedAction::CopyExport, "r1");
    done.event_id = "c1-done".into();
    done.payload.outcome = Some(ActionOutcome::Done);
    log.record(done).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);
}

#[test]
fn an_open_copy_action_is_one_that_still_owes_a_durable_result() {
    let mut log = ActionEventLog::new();
    log.record(event("c1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);

    let mut pending = event("c1", ProtectedAction::CopyExport, "r1");
    pending.event_id = "c1-pending".into();
    pending.payload.outcome = Some(ActionOutcome::Done);
    pending.payload.pending = Some(PendingResult::CopyManifestPending);
    log.record(pending).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);

    let mut closed = event("c1", ProtectedAction::CopyExport, "r1");
    closed.event_id = "c1-closed".into();
    closed.payload.outcome = Some(ActionOutcome::Done);
    log.record(closed).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 0);
}

#[test]
fn subject_export_waits_for_the_copy_controls() {
    let controls = BindingControls::default();
    assert_eq!(
        check_binding(&subject_scope(), ProtectedAction::CopyExport, &controls),
        Err(BindingRefusal::CopyControlsNotAccepted)
    );
    let accepted = BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    };
    assert!(check_binding(&subject_scope(), ProtectedAction::CopyExport, &accepted).is_ok());
}

#[test]
fn request_bound_erasure_waits_for_the_guarded_restore_and_the_erasure_commands() {
    assert_eq!(
        check_binding(
            &subject_scope(),
            ProtectedAction::Erasure,
            &BindingControls::default()
        ),
        Err(BindingRefusal::GuardedRestoreNotReady)
    );
    let restored_only = BindingControls {
        guarded_restore_ready: true,
        ..BindingControls::default()
    };
    assert_eq!(
        check_binding(&subject_scope(), ProtectedAction::Erasure, &restored_only),
        Err(BindingRefusal::ErasureCommandsNotReady)
    );
    let ready = BindingControls {
        guarded_restore_ready: true,
        erasure_commands_ready: true,
        ..BindingControls::default()
    };
    assert!(check_binding(&subject_scope(), ProtectedAction::Erasure, &ready).is_ok());
}

#[test]
fn subject_scope_restore_is_gated_by_the_restore_check_not_the_binding_gate() {
    let controls = BindingControls::default();
    assert!(check_binding(&subject_scope(), ProtectedAction::Restore, &controls).is_ok());
}

#[test]
fn bank_scope_actions_are_not_gated() {
    let controls = BindingControls::default();
    for action in [
        ProtectedAction::CopyExport,
        ProtectedAction::Erasure,
        ProtectedAction::Restore,
    ] {
        assert!(check_binding(&bank_scope(), action, &controls).is_ok());
    }
}

#[test]
fn restore_refusals_are_typed_and_ordered() {
    let inputs = RestoreInputs {
        manifest_present: false,
        digest_verified: false,
        receiver_matches: false,
    };
    assert_eq!(
        check_restore(&inputs),
        Err(RestoreRefusal::CopyManifestMissing)
    );
    let inputs = RestoreInputs {
        manifest_present: true,
        ..inputs
    };
    assert_eq!(
        check_restore(&inputs),
        Err(RestoreRefusal::CopyDigestMismatch)
    );
    let inputs = RestoreInputs {
        digest_verified: true,
        ..inputs
    };
    assert_eq!(
        check_restore(&inputs),
        Err(RestoreRefusal::ReceiverMismatch)
    );
    let inputs = RestoreInputs {
        receiver_matches: true,
        ..inputs
    };
    assert!(check_restore(&inputs).is_ok());
}

#[test]
fn closure_acceptance_composes_with_the_open_copy_count_from_the_log() {
    let mut log = ActionEventLog::new();
    log.record(event("c1", ProtectedAction::CopyExport, "r1"))
        .unwrap();
    let blocked = ClosureInputs {
        product_path_known: true,
        open_copy_actions: log.open_copy_actions("r1"),
        retention_exception_approved: false,
    };
    assert_eq!(accept_closure(&blocked), Err(ClosureBlock::OpenCopyAction));
    let mut done = event("c1", ProtectedAction::CopyExport, "r1");
    done.event_id = "c1-done".into();
    done.payload.outcome = Some(ActionOutcome::Done);
    log.record(done).unwrap();
    let clear = ClosureInputs {
        open_copy_actions: log.open_copy_actions("r1"),
        ..blocked
    };
    assert_eq!(clear.open_copy_actions, 0);
    assert!(accept_closure(&clear).is_ok());
}

#[test]
fn an_unknown_product_path_blocks_technical_completion() {
    // The unknown path is checked before open copies: it wins even when
    // copies are still open.
    for open in [0, 2] {
        let inputs = ClosureInputs {
            product_path_known: false,
            open_copy_actions: open,
            retention_exception_approved: false,
        };
        assert_eq!(
            accept_closure(&inputs),
            Err(ClosureBlock::UnknownProductPath),
            "{open} open copies"
        );
    }
}

#[test]
fn an_open_copy_action_blocks_closure() {
    let inputs = ClosureInputs {
        product_path_known: true,
        open_copy_actions: 1,
        retention_exception_approved: false,
    };
    assert_eq!(accept_closure(&inputs), Err(ClosureBlock::OpenCopyAction));
}

#[test]
fn an_approved_retention_exception_yields_a_partial_result() {
    let inputs = ClosureInputs {
        product_path_known: true,
        open_copy_actions: 0,
        retention_exception_approved: true,
    };
    assert_eq!(
        accept_closure(&inputs).unwrap(),
        crate::subject_request::FinalOutcome::PartiallyCompleted
    );
    let without = ClosureInputs {
        retention_exception_approved: false,
        ..inputs
    };
    assert_eq!(
        accept_closure(&without).unwrap(),
        crate::subject_request::FinalOutcome::Completed
    );
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
    let mut owed = event("c1", ProtectedAction::CopyExport, "r1");
    owed.event_id = "c1-owed".into();
    owed.payload.outcome = Some(ActionOutcome::Done);
    owed.payload.pending = Some(PendingResult::OutcomePending);
    log.record(owed).unwrap();
    assert_eq!(log.open_copy_actions("r1"), 1);
    let mut closed = event("c1", ProtectedAction::CopyExport, "r1");
    closed.event_id = "c1-closed".into();
    closed.payload.outcome = Some(ActionOutcome::Done);
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
        let mut terminal = event("c1", ProtectedAction::CopyExport, "r1");
        terminal.event_id = format!("c1-{index}");
        terminal.payload.outcome = Some(outcome);
        terminal.payload.pending = pending;
        log.record(terminal).unwrap();
        assert_eq!(log.open_copy_actions("r1"), open);
    }
}
