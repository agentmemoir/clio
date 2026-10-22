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

/// Return a captured snapshot only after a competing service call commits.
struct InterleavedStore<'a> {
    store: &'a clio_store::SqliteStore,
    first_load: std::sync::atomic::AtomicBool,
    competing_close: bool,
}

impl DurableRequestStore for InterleavedStore<'_> {
    fn compare_and_save(
        &self,
        expected: Option<&clio_store::DurableRequestRow>,
        row: &clio_store::DurableRequestRow,
    ) -> Result<bool, clio_types::AmError> {
        self.store.compare_and_save(expected, row)
    }
    fn save(&self, row: &clio_store::DurableRequestRow) -> Result<(), clio_types::AmError> {
        self.store.save(row)
    }

    fn load(
        &self,
        request_id: &str,
    ) -> Result<Option<clio_store::DurableRequestRow>, clio_types::AmError> {
        let snapshot = self.store.load(request_id)?;
        if self
            .first_load
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            if self.competing_close {
                close(&env(self.store), &ctx("bank-a"), request_id, "product")?;
            } else {
                complete_path(&env(self.store), &ctx("bank-a"), request_id, "other")?;
            }
        }
        Ok(snapshot)
    }

    fn load_all(&self) -> Result<Vec<clio_store::DurableRequestRow>, clio_types::AmError> {
        self.store.load_all()
    }
}

#[test]
fn completion_preserves_an_acknowledged_competing_path() {
    let store = store();
    persist(&store, &record("request", RequestState::InProgress));
    let interleaved = InterleavedStore {
        store: &store,
        first_load: true.into(),
        competing_close: false,
    };
    let mut service = env(&store);
    service.durable = &interleaved;
    let outcome = complete_path(&service, &ctx("bank-a"), "request", "product");
    let saved = row_to_record(&store.load("request").unwrap().unwrap()).unwrap();
    assert_eq!(
        saved.paths.get("other").map(|slot| &slot.state),
        Some(&PathState::Done)
    );
    if outcome.is_ok() {
        assert_eq!(saved.paths["product"].state, PathState::Done);
    } else {
        assert!(!saved.paths.contains_key("product"));
    }
}

#[test]
fn completion_cannot_reopen_a_competing_final_close() {
    let store = store();
    let mut request = record("request", RequestState::InProgress);
    request.paths.insert(
        "product".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    persist(&store, &request);
    let interleaved = InterleavedStore {
        store: &store,
        first_load: true.into(),
        competing_close: true,
    };
    let mut service = env(&store);
    service.durable = &interleaved;
    let outcome = complete_path(&service, &ctx("bank-a"), "request", "later");
    let saved = row_to_record(&store.load("request").unwrap().unwrap()).unwrap();
    assert_eq!(saved.state, RequestState::Final(FinalOutcome::Completed));
    assert!(
        outcome.is_err(),
        "stale completion must not acknowledge a write"
    );
    assert!(!saved.paths.contains_key("later"));
}

#[test]
fn close_preserves_an_acknowledged_competing_path() {
    let store = store();
    let mut request = record("request", RequestState::InProgress);
    request.paths.insert(
        "product".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    persist(&store, &request);
    let interleaved = InterleavedStore {
        store: &store,
        first_load: true.into(),
        competing_close: false,
    };
    let mut service = env(&store);
    service.durable = &interleaved;
    let outcome = close(&service, &ctx("bank-a"), "request", "product");
    let saved = row_to_record(&store.load("request").unwrap().unwrap()).unwrap();
    assert_eq!(saved.paths["other"].state, PathState::Done);
    if outcome.is_err() {
        assert_eq!(saved.state, RequestState::InProgress);
    } else {
        assert_eq!(saved.state, RequestState::Final(FinalOutcome::Completed));
    }
}
