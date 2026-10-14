// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request state
//! contract: the total transition table and the ledger apply commands.
//!
//! ## Owns
//! - Totality, idempotency, and category assertions for `transition`
//! - Apply-command assertions for `SubjectRequestLedger`
//!
//! ## Does not own
//! - Register binding and replay assertions
//!   (`subject_request_register_tests.rs`)
//! - The subject-request contract itself (see `subject_request.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::{
    FinalOutcome, RequestCommand, RequestKind, RequestScope, RequestState, StateCategory,
    SubjectRequestLedger, transition,
};
use clio_types::ErrorCode;

fn subject_scope() -> RequestScope {
    RequestScope::Subject {
        bank_id: "bank-a".into(),
        subject_id: "subject-1".into(),
    }
}

fn refuse_command() -> RequestCommand {
    RequestCommand::Refuse {
        rationale: "r".into(),
    }
}

fn block_command() -> RequestCommand {
    RequestCommand::Block { reason: "e".into() }
}

fn refused_state(rationale: &str) -> RequestState {
    RequestState::Refused {
        rationale: rationale.into(),
    }
}

fn ledger_in(setup: &str) -> SubjectRequestLedger {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    if setup == "received" {
        return ledger;
    }
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    if setup == "in_progress" {
        return ledger;
    }
    if setup == "blocked" {
        ledger.apply("r1", &block_command()).unwrap();
        return ledger;
    }
    ledger
        .apply("r1", &RequestCommand::MarkTechnicalComplete)
        .unwrap();
    if setup == "technical" {
        return ledger;
    }
    ledger.apply("r1", &RequestCommand::AwaitOperator).unwrap();
    ledger
}

fn expected_next(state: &RequestState, command: &RequestCommand) -> Option<RequestState> {
    match state {
        RequestState::Received => match command {
            RequestCommand::Begin => Some(RequestState::InProgress),
            RequestCommand::Refuse { rationale } => Some(refused_state(rationale)),
            _ => None,
        },
        RequestState::InProgress => match command {
            RequestCommand::MarkTechnicalComplete => Some(RequestState::TechnicalComplete),
            RequestCommand::Block { reason } => Some(RequestState::Blocked {
                reason: reason.clone(),
            }),
            RequestCommand::Refuse { rationale } => Some(refused_state(rationale)),
            _ => None,
        },
        RequestState::Blocked { .. } => match command {
            RequestCommand::Resume => Some(RequestState::InProgress),
            RequestCommand::Refuse { rationale } => Some(refused_state(rationale)),
            _ => None,
        },
        RequestState::TechnicalComplete => match command {
            RequestCommand::AwaitOperator => Some(RequestState::AwaitingOperator),
            _ => None,
        },
        RequestState::AwaitingOperator => match command {
            RequestCommand::Finalize { outcome } => Some(RequestState::Final(*outcome)),
            RequestCommand::Refuse { rationale } => Some(refused_state(rationale)),
            _ => None,
        },
        RequestState::Final(_) | RequestState::Refused { .. } => None,
    }
}

#[test]
fn the_transition_table_allows_exactly_the_documented_pairs() {
    for state in RequestState::all() {
        for command in RequestCommand::all() {
            match expected_next(&state, &command) {
                Some(next) => assert_eq!(
                    transition(&state, &command).unwrap(),
                    next,
                    "wrong next state for {state:?} with {command:?}"
                ),
                None => assert_eq!(
                    transition(&state, &command).unwrap_err().code,
                    ErrorCode::InvalidArgument,
                    "wrong verdict for {state:?} with {command:?}"
                ),
            }
            if let Ok(next) = transition(&state, &command) {
                assert!(
                    transition(&next, &command).is_err(),
                    "a repeated input produced a second transition: {state:?} with {command:?}"
                );
            }
        }
    }
}

#[test]
fn terminal_states_accept_no_command() {
    let terminal = vec![
        RequestState::Final(FinalOutcome::Completed),
        RequestState::Final(FinalOutcome::PartiallyCompleted),
        RequestState::Refused {
            rationale: "done".into(),
        },
    ];
    for state in terminal {
        for command in RequestCommand::all() {
            let err = transition(&state, &command).expect_err("terminal state moved");
            assert_eq!(err.code, ErrorCode::InvalidArgument, "{state:?}");
        }
    }
}

#[test]
fn state_categories_separate_completion_operator_action_exceptions_and_outcomes() {
    let categories = [
        StateCategory::Working,
        StateCategory::Working,
        StateCategory::TechnicalCompletion,
        StateCategory::OperatorAction,
        StateCategory::Exception,
        StateCategory::FinalOutcome,
        StateCategory::FinalOutcome,
        StateCategory::FinalOutcome,
    ];
    let states = RequestState::all();
    assert_eq!(states.len(), categories.len());
    for (state, category) in states.iter().zip(categories) {
        assert_eq!(state.category(), category, "{state:?}");
    }
}

#[test]
fn a_refused_outcome_records_its_rationale() {
    let next = transition(&RequestState::InProgress, &refuse_command()).unwrap();
    assert_eq!(next, refused_state("r"));
}

#[test]
fn a_request_reaches_a_final_outcome_through_the_operator_gate() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    assert_eq!(
        ledger.apply("r1", &RequestCommand::Begin).unwrap(),
        RequestState::InProgress
    );
    assert_eq!(
        ledger
            .apply("r1", &RequestCommand::MarkTechnicalComplete)
            .unwrap(),
        RequestState::TechnicalComplete
    );
    assert_eq!(
        ledger.apply("r1", &RequestCommand::AwaitOperator).unwrap(),
        RequestState::AwaitingOperator
    );
    let next = ledger
        .apply(
            "r1",
            &RequestCommand::Finalize {
                outcome: FinalOutcome::PartiallyCompleted,
            },
        )
        .unwrap();
    assert_eq!(next, RequestState::Final(FinalOutcome::PartiallyCompleted));
}

fn state_is_listed(state: &RequestState) -> bool {
    match state {
        RequestState::Received
        | RequestState::InProgress
        | RequestState::TechnicalComplete
        | RequestState::AwaitingOperator
        | RequestState::Blocked { .. }
        | RequestState::Final(_)
        | RequestState::Refused { .. } => true,
    }
}

fn command_is_listed(command: &RequestCommand) -> bool {
    match command {
        RequestCommand::Begin
        | RequestCommand::MarkTechnicalComplete
        | RequestCommand::AwaitOperator
        | RequestCommand::Finalize { .. }
        | RequestCommand::Refuse { .. }
        | RequestCommand::Block { .. }
        | RequestCommand::Resume => true,
    }
}

#[test]
fn the_guarded_enumerations_cover_every_state_and_command() {
    assert!(RequestState::all().iter().all(state_is_listed));
    assert!(RequestCommand::all().iter().all(command_is_listed));
    assert_eq!(RequestState::all().len(), 8);
    assert_eq!(RequestCommand::all().len(), 8);
}

#[test]
fn an_allowed_command_applies_once_and_the_repeat_is_refused() {
    let rows: Vec<(&str, RequestCommand)> = vec![
        ("received", RequestCommand::Begin),
        ("received", refuse_command()),
        ("in_progress", RequestCommand::MarkTechnicalComplete),
        ("in_progress", block_command()),
        ("in_progress", refuse_command()),
        ("technical", RequestCommand::AwaitOperator),
        (
            "awaiting",
            RequestCommand::Finalize {
                outcome: FinalOutcome::Completed,
            },
        ),
        (
            "awaiting",
            RequestCommand::Finalize {
                outcome: FinalOutcome::PartiallyCompleted,
            },
        ),
        ("awaiting", refuse_command()),
        ("blocked", RequestCommand::Resume),
        ("blocked", refuse_command()),
    ];
    for (setup, command) in rows {
        let mut ledger = ledger_in(setup);
        let next = ledger.apply("r1", &command).unwrap();
        let err = ledger.apply("r1", &command).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{setup:?}");
        assert_eq!(
            ledger.record("r1").map(|r| r.state.clone()),
            Some(next),
            "{setup:?}"
        );
    }
}

#[test]
fn a_refused_command_leaves_the_stored_state_unchanged() {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register("r1", RequestKind::Erasure, &subject_scope(), &[])
        .unwrap();
    ledger
        .apply(
            "r1",
            &RequestCommand::Refuse {
                rationale: "withdrawn".into(),
            },
        )
        .unwrap();
    let err = ledger.apply("r1", &RequestCommand::Begin).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        ledger.record("r1").map(|r| r.state.clone()),
        Some(RequestState::Refused {
            rationale: "withdrawn".into()
        })
    );
}

#[test]
fn apply_reports_an_unknown_request_id() {
    let mut ledger = SubjectRequestLedger::new();
    let err = ledger.apply("missing", &RequestCommand::Begin).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
