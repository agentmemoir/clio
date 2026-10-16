// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the key-loss race:
//! a destroyed key whose purge never committed stays pending until a pass
//! commits the purge, and a duplicate or concurrent call creates no new key or
//! second effect.
//!
//! ## Owns
//! - The Postgres raced-key-loss and duplicate-call assertions
//! - The racing-intent event-writer double
//!
//! ## Does not own
//! - The retry contract (`erasure_retry.rs`) and the reconcile
//!   (`erasure_reconcile.rs`)
//! - The shared SQLite fixtures (`subject_request_store_wiring_tests.rs`)
//! - The Postgres retry helpers (`erasure_retry_pg_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Tests here break when a raced Postgres retry loses the key twice, refuses a
//! concurrent winner, or settles over an uncommitted purge.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::EraseVecSink;
use crate::erasure_reconcile::reconcile_erasure_actions;
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::subject_request::SubjectRequestRecord;
use crate::subject_request_events::ProtectedAction;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::erasure_retry_pg_tests::{
    pg_action_rows, pg_live_set, pg_tombstone_count,
};
use crate::subject_request_store_wiring::erasure_retry_reconcile_tests::resume_facts;
use crate::subject_request_store_wiring::erasure_retry_tests::Attempt;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::pg_fixture_or_skip;
use crate::subject_request_store_wiring::subject_request_store_wiring_tests::{
    RetryStore, persist, started_record, wiring_item,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    PostgresStore, Store,
};
use clio_types::{AmError, ErrorCode};
use std::sync::atomic::Ordering;

const NOW: &str = "2026-01-01T00:00:00Z";

/// Record one pending erasure intent for `request_id` over the real Postgres
/// store.
fn pg_record_erasure_intent(store: &PostgresStore, request_id: &str) {
    let producer = ProtectedAction::Erasure.producer_id();
    store
        .record_intent(&LocalEventIntent {
            action_id: &erasure_action_id(request_id),
            actor: "harness",
            scope: producer,
            kind: producer,
            event_time: NOW,
        })
        .expect("the pending intent records");
}

/// A writer that loses the intent race: the row lands, then the duplicate write
/// fails as a concurrent caller would see it.
struct PgRacingIntent<'a> {
    inner: &'a dyn LocalEventStore,
}

impl LocalEventStore for PgRacingIntent<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        self.inner.record_intent(intent)?;
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            "injected duplicate intent",
        ))
    }

    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        self.inner.record_outcome(action_id, denied, result)
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.inner.pending_actions()
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.inner.list_events()
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.inner.prune_completed_before(cutoff)
    }
}

/// A destroyed key with an uncommitted purge stays pending across passes, and
/// is settled by the pass that finally commits the purge.
#[test]
fn postgres_reconcile_keeps_a_destroyed_key_without_the_purge_pending() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    pg_record_erasure_intent(&fixture.store, &attempt.request_id);
    let mut double = RetryStore::failing();
    double.banks = vec![attempt.bank.clone()];
    let sink = EraseVecSink::default();
    let env = attempt.env(&double, &sink);
    let live = double.effect_bank_set();
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let action_id = erasure_action_id(&attempt.request_id);
    let first = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(first.pending, vec![action_id.clone()]);
    assert!(first.settled.is_empty(), "{first:?}");
    assert!(
        double.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the first pass destroyed the key without committing the purge"
    );
    assert_eq!(
        double.erase_calls(),
        1,
        "the first pass runs the effect once"
    );
    // The second pass starts from a destroyed key and an uncommitted purge: it
    // must retry the purge instead of settling a completion over it.
    let second = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(second.pending, vec![action_id.clone()], "still pending");
    assert!(second.settled.is_empty(), "{second:?}");
    let rows = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(double.erase_calls(), 2, "the second pass retries the purge");
    double.fail.store(false, Ordering::SeqCst);
    let third = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(
        third.settled,
        vec![action_id.clone()],
        "the pass that commits the purge settles"
    );
    let settled = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(settled.len(), 1, "exactly one outcome");
    assert_eq!(settled[0].state, LocalEventState::Allowed);
    assert_eq!(settled[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert_eq!(double.erase_calls(), 3, "each pass runs the purge once");
}

/// A concurrent caller that won the intent write is resumed, not refused, and
/// the race runs one effect: the other subject stays readable.
#[test]
fn postgres_a_racing_intent_write_resumes_without_a_second_effect() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    fixture
        .store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let other = wiring_item(&attempt.bank, &unique("other-subject"));
    fixture
        .store
        .create_memory_item(&other, "tester")
        .expect("seed the other subject's item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let live = pg_live_set(&attempt, &fixture.store);
    let events = PgRacingIntent {
        inner: &fixture.store,
    };
    let outcome =
        erase_confirmed_request(&fixture.store, &events, &attempt.request_id, &args, &live)
            .expect("the raced intent resumes the flow");
    assert_eq!(outcome.status, "COMPLETED");
    assert!(
        fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .expect("key"),
        "the completed race destroys the key, as the SQLite twin pins"
    );
    let rows = pg_action_rows(&fixture.store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    let tombstones = pg_tombstone_count(&attempt, &fixture.store);
    assert!(tombstones > 0, "the race ran the effect once");
    assert!(
        fixture
            .store
            .get_memory_item(&attempt.bank, &other.id)
            .expect("readable")
            .is_some(),
        "the other subject remains readable"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "the race creates no second effect"
    );
}

/// A duplicate call after completion adds no second effect and does not
/// recreate the destroyed key.
#[test]
fn postgres_a_duplicate_call_after_completion_adds_no_second_effect() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    fixture
        .store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let live = pg_live_set(&attempt, &fixture.store);
    erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the first attempt completes");
    let tombstones = pg_tombstone_count(&attempt, &fixture.store);
    let duplicate = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("a duplicate call is idempotent");
    assert_eq!(duplicate.status, "COMPLETED");
    assert!(
        !fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .map(|destroyed| !destroyed)
            .expect("key"),
        "the duplicate recreates no key"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "the duplicate adds no second effect"
    );
    assert_eq!(
        pg_action_rows(&fixture.store, &erasure_action_id(&attempt.request_id)).len(),
        1,
        "one allowed outcome, never a second"
    );
}
