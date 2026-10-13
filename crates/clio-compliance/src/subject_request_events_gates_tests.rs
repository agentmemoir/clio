// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request
//! action gates: the public-binding gate, the restore gate, and the closure
//! acceptance check, including its composition with the log's open-copy
//! count.
//!
//! ## Owns
//! - Binding, restore, and closure acceptance assertions
//!
//! ## Does not own
//! - The gates themselves (see `subject_request_events.rs`)
//! - Log record and idempotency coverage (see `subject_request_events_tests.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::action_event_log::ActionEventLog;
use crate::subject_request::RequestScope;
use crate::subject_request_events::{
    ActionOutcome, BindingControls, BindingRefusal, ClosureBlock, ClosureInputs, ProtectedAction,
    RestoreInputs, RestoreRefusal, accept_closure, check_binding, check_restore,
};
use crate::subject_request_events_tests::{event, subject_scope, terminal};

fn bank_scope() -> RequestScope {
    RequestScope::Bank {
        bank_id: "bank-a".into(),
    }
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
    let done = terminal(
        "c1-done",
        ProtectedAction::CopyExport,
        "r1",
        ActionOutcome::Done,
        None,
    );
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
