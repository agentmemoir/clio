// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twins of the confirmed
//! erasure retry fault classes and its restart reconciliation: the intent-write
//! refusal that leaves no denial or key effect, the same-scope repeat and the
//! foreign-kind refusal, a hard effect error that stays pending until a resume
//! completes, and an outcome failure that reconciles across a real restart.
//!
//! ## Owns
//! - The Postgres retry-flow assertions and the local effect-fault double
//!
//! ## Does not own
//! - The retry contract (`erasure_retry.rs`) and the reconcile
//!   (`erasure_reconcile.rs`)
//! - The shared SQLite fixtures (`erasure_retry_tests.rs`,
//!   `subject_request_store_wiring_tests.rs`)
//! - The private-schema Postgres fixture and its skip note
//!   (`subject_request_store_wiring_pg_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres retry stops matching the SQLite contract
//! for a failed intent or a failed outcome.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::EraseVecSink;
use crate::erasure_reconcile::reconcile_erasure_actions;
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::restore_guard::RESTORE_EVENT_KIND;
use crate::subject_request::SubjectRequestRecord;
use crate::subject_request_store_wiring::erasure_retry_reconcile_tests::resume_facts;
use crate::subject_request_store_wiring::erasure_retry_tests::Attempt;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::pg_fixture_or_skip;
use crate::subject_request_store_wiring::subject_request_store_wiring_tests::{
    persist, started_record, wiring_item,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, DurableRequestRow, DurableRequestStore, ItemIdentity,
    LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView, PostgresStore, Store,
    SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{AmError, ErrorCode};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Timestamp every test's durable write is stamped with.
const NOW: &str = "2026-01-01T00:00:00Z";

/// A compliance-store double whose effect fails without marking the key
/// destroyed, so the flow sees a hard effect error and never a key loss.
struct PgEffectFault {
    fail_effect: AtomicBool,
    erase_calls: AtomicUsize,
}

impl PgEffectFault {
    fn new() -> Self {
        Self {
            fail_effect: AtomicBool::new(true),
            erase_calls: AtomicUsize::new(0),
        }
    }
}

impl ComplianceStore for PgEffectFault {
    fn list_subject_items(&self, _: &str, _: &str) -> Result<Vec<ItemIdentity>, AmError> {
        Ok(vec![])
    }

    fn erase_subject(&self, _: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_effect.load(Ordering::SeqCst) {
            return Err(AmError::new(ErrorCode::Internal, "injected effect failure"));
        }
        Ok(SubjectEraseOutcome {
            item_ids: vec![],
            categories: vec![],
            item_hashes: vec![],
            subject_hash: "subject-hash".into(),
            vectors_purged: 0,
            assoc_edges_purged: 0,
            memtree_nodes_purged: 0,
            triples_purged: 0,
            derived_items_purged: 0,
            persona_context_rows_purged: 0,
            tombstones_written: 0,
        })
    }

    fn subject_dek_destroyed(&self, _: &str) -> Result<bool, AmError> {
        Ok(false)
    }

    fn read_tombstones(&self, _: &str, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }

    fn list_tombstones(&self, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }
}

/// The live bank set one attempt's effect fans out to, read from the store.
pub(super) fn pg_live_set(attempt: &Attempt, store: &PostgresStore) -> Vec<String> {
    store
        .erase_bank_set(&attempt.bank, &attempt.subject)
        .expect("live bank set")
}

/// Every event row one action id holds on the Postgres store.
pub(super) fn pg_action_rows(store: &PostgresStore, action_id: &str) -> Vec<LocalEventView> {
    store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect()
}

/// The durable row a request id currently holds on the Postgres store.
fn pg_load_row(store: &PostgresStore, request_id: &str) -> DurableRequestRow {
    store.load(request_id).expect("load").expect("row exists")
}

/// The committed tombstone count one request's erasure wrote.
pub(super) fn pg_tombstone_count(attempt: &Attempt, store: &PostgresStore) -> usize {
    store
        .read_tombstones(&attempt.bank, &attempt.request_id)
        .expect("tombstones")
        .len()
}

/// A failed intent write refuses before any erasure effect and leaves no
/// denial, no key loss, and no durable intent row.
#[test]
fn postgres_an_intent_write_failure_refuses_before_the_effect() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let live = pg_live_set(&attempt, &fixture.store);
    let events = FaultEvents::intent_fails();
    let err = erase_confirmed_request(
        &fixture.store,
        &events,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect_err("a failed intent must refuse");
    assert_eq!(err.code, ErrorCode::Internal, "the injected intent fault");
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        0,
        "a refused intent runs no store effect"
    );
    assert!(
        !attempt.journal.exists(),
        "a failed intent records no journal denial"
    );
    assert!(
        !fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .expect("key"),
        "a failed intent loses no key"
    );
    assert!(
        fixture.store.pending_actions().expect("pending").is_empty(),
        "a failed intent leaves no durable action"
    );
}

/// A same-scope repeat is idempotent: one allowed outcome and no second store
/// effect, and the key stays destroyed.
#[test]
fn postgres_a_same_scope_repeat_is_idempotent() {
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
    let first = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the first attempt completes");
    assert_eq!(first.status, "COMPLETED");
    let tombstones = pg_tombstone_count(&attempt, &fixture.store);
    // The SQLite twin pins the exact count; one subject erase writes one
    // tombstone here, so the count is pinned exactly.
    assert_eq!(tombstones, 1, "the first attempt commits the purge once");
    let repeat = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("a same-scope repeat is idempotent");
    assert_eq!(repeat.status, "COMPLETED");
    assert!(
        fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .expect("key"),
        "the key stays destroyed"
    );
    let rows = pg_action_rows(&fixture.store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows.len(), 1, "one allowed outcome, never a second");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "the repeat repeats no store work"
    );
}

/// A same-id row of another event kind refuses and is never mutated.
#[test]
fn postgres_an_intent_id_held_by_another_event_kind_refuses() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    let action_id = erasure_action_id(&attempt.request_id);
    fixture
        .store
        .record_intent(&LocalEventIntent {
            action_id: &action_id,
            actor: "harness",
            scope: RESTORE_EVENT_KIND,
            kind: RESTORE_EVENT_KIND,
            event_time: NOW,
        })
        .expect("the foreign intent records");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let live = pg_live_set(&attempt, &fixture.store);
    let err = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect_err("a foreign-kind row must refuse the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    let rows = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(rows.len(), 1, "the foreign row stays the only row");
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert_eq!(
        rows[0].kind, RESTORE_EVENT_KIND,
        "the foreign row is never mutated"
    );
}

/// A hard effect error with no key loss stays pending through the flow and the
/// restart pass, and a later resume completes it with one allowed outcome.
#[test]
fn postgres_a_harmless_effect_failure_keeps_the_intent_pending_until_a_resume_completes() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    let double = PgEffectFault::new();
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&double, &ctx, &sink);
    let live = vec![attempt.bank.clone()];
    let err = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect_err("the hard effect error is reported");
    assert_eq!(
        err.message, "injected effect failure",
        "the original error returns"
    );
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        1,
        "the effect ran once"
    );
    let action_id = erasure_action_id(&attempt.request_id);
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![action_id.clone()],
        "the action stays pending"
    );
    let rows = pg_action_rows(&fixture.store, &action_id);
    assert!(rows[0].result.is_none(), "no outcome, never a denial");
    let env = attempt.env(&double, &fixture.store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(report.pending, vec![action_id.clone()]);
    double.fail_effect.store(false, Ordering::SeqCst);
    let recovered = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the resume completes");
    assert_eq!(recovered.status, "COMPLETED");
    let settled = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(settled.len(), 1, "exactly one outcome");
    assert_eq!(settled[0].state, LocalEventState::Allowed);
}

/// An outcome failure leaves one durable pending action that reconciles
/// across a real Postgres restart without another erase.
#[test]
fn postgres_an_outcome_failure_reconciles_across_a_restart_without_another_erase() {
    let Some(mut fixture) = pg_fixture_or_skip() else {
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
    let live = pg_live_set(&attempt, &fixture.store);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let events = FaultEvents::outcome_fails_over(&fixture.store);
    erase_confirmed_request(
        &fixture.store,
        &events,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect_err("the failed outcome write is reported");
    let action_id = erasure_action_id(&attempt.request_id);
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![action_id.clone()],
        "exactly one pending action remains"
    );
    let pending_row = pg_load_row(&fixture.store, &attempt.request_id);
    let tombstones = pg_tombstone_count(&attempt, &fixture.store);
    // The restart boundary: reopen the same scoped schema before reconciling.
    fixture.restart();
    assert_eq!(
        pg_load_row(&fixture.store, &attempt.request_id),
        pending_row,
        "the restart leaves the pending row unchanged"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "the restart leaves the tombstone count unchanged"
    );
    let changed = vec![attempt.bank.clone(), unique("joined-bank")];
    let sink = EraseVecSink::default();
    let env = attempt.env(&fixture.store, &fixture.store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, changed.clone()));
    let report = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(report.settled, vec![action_id.clone()]);
    let rows = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(
        rows[0].result.as_deref(),
        Some(OUTCOME_COMPLETED),
        "the settle never claims no effect"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "reconciliation repeats no key loss or store work"
    );
    let again =
        reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume).expect("again");
    assert_eq!(
        again,
        crate::erasure_reconcile::ErasureReconcileReport::default()
    );
}
