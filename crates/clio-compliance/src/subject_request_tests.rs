// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the subject-request
//! contract: scope binding, the total transition table, and the ledger
//! register/apply commands.
//!
//! ## Owns
//! - Scope replay and conflict assertions for `SubjectRequestLedger`
//! - Totality, idempotency, and category assertions for `transition`
//!
//! ## Does not own
//! - The subject-request contract itself (see `subject_request.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::{
    FinalOutcome, RegisterOutcome, RequestCommand, RequestScope, RequestState, StateCategory,
    SubjectRequestLedger, SubjectRequestRecord, transition,
};
use clio_types::ErrorCode;

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
    ledger.register("r1", &subject_scope()).unwrap();
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

#[test]
fn register_opens_a_received_record_with_the_given_scope() {
    let mut ledger = SubjectRequestLedger::new();
    let outcome = ledger.register("r1", &subject_scope()).unwrap();
    assert_eq!(outcome, RegisterOutcome::Opened);
    let record = ledger.record("r1").expect("record stored");
    assert_eq!(record.scope, subject_scope());
    assert_eq!(record.state, RequestState::Received);
}

#[test]
fn same_id_same_scope_replays_the_stored_result_and_applies_nothing() {
    let mut ledger = SubjectRequestLedger::new();
    ledger.register("r1", &subject_scope()).unwrap();
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    ledger
        .apply("r1", &RequestCommand::MarkTechnicalComplete)
        .unwrap();
    let outcome = ledger.register("r1", &subject_scope()).unwrap();
    let replayed = RegisterOutcome::Replayed {
        state: RequestState::TechnicalComplete,
    };
    assert_eq!(outcome, replayed);
    assert_eq!(
        ledger.record("r1").map(|r| r.state.clone()),
        Some(RequestState::TechnicalComplete)
    );
}

#[test]
fn same_id_changed_scope_is_refused_without_state_change() {
    let mut ledger = SubjectRequestLedger::new();
    ledger.register("r1", &bank_scope()).unwrap();
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    let err = ledger.register("r1", &subject_scope()).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        ledger.record("r1").map(|r| r.state.clone()),
        Some(RequestState::InProgress)
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
        ledger.register("r1", &subject_scope()).unwrap();
        ledger.register("r2", &other).unwrap();
        ledger.apply("r1", &RequestCommand::Begin).unwrap();
        assert_eq!(
            ledger.record("r2").map(|r| r.state.clone()),
            Some(RequestState::Received),
            "{other:?}"
        );
        let err = ledger.register("r1", &other).unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden, "{other:?}");
        assert_eq!(
            ledger
                .record("r1")
                .map(|r| (r.scope.clone(), r.state.clone())),
            Some((subject_scope(), RequestState::InProgress)),
            "{other:?}"
        );
    }
}

#[test]
fn reloaded_records_replay_after_a_modeled_restart() {
    let mut ledger = SubjectRequestLedger::new();
    ledger.register("r1", &subject_scope()).unwrap();
    ledger.apply("r1", &RequestCommand::Begin).unwrap();
    let bytes = serde_json::to_string(&ledger.records()).expect("serialize records");
    let reloaded: Vec<SubjectRequestRecord> =
        serde_json::from_str(&bytes).expect("deserialize records");
    let mut restarted = SubjectRequestLedger::load(reloaded);
    let outcome = restarted.register("r1", &subject_scope()).unwrap();
    let replayed = RegisterOutcome::Replayed {
        state: RequestState::InProgress,
    };
    assert_eq!(outcome, replayed);
    let other = subject_in("bank-a", "subject-2");
    let err = restarted.register("r1", &other).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        restarted.record("r1").map(|r| r.scope.clone()),
        Some(subject_scope())
    );
    assert_eq!(
        restarted.record("r1").map(|r| r.state.clone()),
        Some(RequestState::InProgress)
    );
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
    ledger.register("r1", &subject_scope()).unwrap();
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
    ledger.register("r1", &subject_scope()).unwrap();
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

#[test]
fn register_validates_the_id_and_the_scope() {
    let mut ledger = SubjectRequestLedger::new();
    let err = ledger.register("  ", &subject_scope()).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = ledger
        .register(
            "r1",
            &RequestScope::Bank {
                bank_id: String::new(),
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
    let err = ledger
        .register(
            "r1",
            &RequestScope::Subject {
                bank_id: "bank-a".into(),
                subject_id: "   ".into(),
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(ledger.record("r1").is_none());
}
