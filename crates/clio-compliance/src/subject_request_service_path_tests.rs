// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//! Test explicit durable product-path completion and its authority boundary.
//! ## Owns
//! Completion lifecycle and refusal assertions.
//! ## Does not own
//! Request bindings or shared fixtures.
//! ## Boundary
//! Changes when completion rules change. Test-only.

use crate::subject_request::{FinalOutcome, PathOwner, PathSlot, PathState, RequestState};
use crate::subject_request_service::{close, complete_path};
use crate::subject_request_service_fixtures::{ctx, env, persist, record, store};
use crate::subject_request_store_wiring::row_to_record;
use clio_store::DurableRequestStore;
use clio_types::{Actor, Bank, ErrorCode, OpContext};

#[test]
fn completion_persists_done_and_enables_close_without_mutating_other_paths() {
    let store = store();
    let mut request = record("request", RequestState::InProgress);
    request.paths.insert(
        "pending".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Pending,
        },
    );
    persist(&store, &request);
    let before = store.load("request").unwrap();
    assert!(close(&env(&store), &ctx("bank-a"), "request", "pending").is_err());
    assert!(close(&env(&store), &ctx("bank-a"), "request", "unknown").is_err());
    assert_eq!(store.load("request").unwrap(), before);
    complete_path(&env(&store), &ctx("bank-a"), "request", "product").unwrap();
    complete_path(&env(&store), &ctx("bank-a"), "request", "product").unwrap();
    let row = store.load("request").unwrap().unwrap();
    let reloaded = row_to_record(&row).unwrap();
    assert_eq!(reloaded.paths["product"].state, PathState::Done);
    assert_eq!(reloaded.paths["pending"].state, PathState::Pending);
    let report = close(&env(&store), &ctx("bank-a"), "request", "product").unwrap();
    assert_eq!(report.state, RequestState::Final(FinalOutcome::Completed));
}

#[test]
fn completion_refuses_unauthorized_invalid_and_terminal_calls_without_writes() {
    let store = store();
    let mut request = record("request", RequestState::InProgress);
    request.paths.insert(
        "external".into(),
        PathSlot {
            owner: PathOwner::External,
            state: PathState::Pending,
        },
    );
    persist(&store, &request);
    let before = store.load("request").unwrap();
    let agent = OpContext::new(Bank::new("bank-a"), Actor::Agent);
    for (context, id, path, code) in [
        (agent, "request", "product", ErrorCode::Forbidden),
        (ctx("other"), "request", "product", ErrorCode::Forbidden),
        (ctx("bank-a"), "request", "external", ErrorCode::Forbidden),
        (ctx("bank-a"), "request", "  ", ErrorCode::InvalidArgument),
        (ctx("bank-a"), "missing", "product", ErrorCode::NotFound),
    ] {
        assert_eq!(
            complete_path(&env(&store), &context, id, path)
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(store.load("request").unwrap(), before);
    }
    for state in [
        RequestState::Final(FinalOutcome::Completed),
        RequestState::Refused {
            rationale: "refused".into(),
        },
    ] {
        request.state = state;
        persist(&store, &request);
        let before = store.load("request").unwrap();
        assert_eq!(
            complete_path(&env(&store), &ctx("bank-a"), "request", "product")
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(store.load("request").unwrap(), before);
    }
}
