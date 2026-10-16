// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the confirmed-erasure flow coverage: the
//! intent-before-denial ordering, the intent-failure and intent-resume
//! classes, and a retried request's idempotency, plus the file-backed scenario
//! fixture the other retry suites reuse.
//!
//! ## Owns
//! - The flow assertions and the shared file-backed scenario fixture
//! - The racing telemetry sink and the shared live-bank-set read
//! - The declaration of the peer-evidence coverage it hosts
//!
//! ## Does not own
//! - The retry contract itself (`erasure_retry.rs`)
//! - The peer-evidence settle rules (`erasure_retry_peer_evidence_tests.rs`)
//! - The reconcile and guard suites (`erasure_retry_reconcile_tests.rs`)
//! - The shared fixtures and erase double (`subject_request_store_wiring_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a retried erasure effects before it refuses or repeats done work.
//!
//! Test-only. Never compiled into production builds.

use super::subject_request_store_wiring_tests::{
    Scratch, open_sqlite, persist, scope_parts, started_record, subject_scope, wiring_item,
};
use crate::erase::{EraseArgs, EraseEvent, EraseSink, EraseVecSink};
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::inventory_fixture::FixtureDir;
use crate::restore_guard::RESTORE_EVENT_KIND;
use crate::subject_peer::{begin_denial, mark_acknowledged};
use crate::subject_request::{PeerState, RequestScope};
use crate::subject_request_store_wiring::record_to_row;
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, DurableRequestRow, DurableRequestStore, LocalEventIntent, LocalEventState,
    LocalEventStore, LocalEventView, SqliteStore, Store,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use std::path::PathBuf;

const NOW: &str = "2026-01-01T00:00:00Z";

/// One file-backed scenario: the scratch store, the request, the journal.
pub(super) struct Attempt {
    scratch: Scratch,
    pub(super) request_id: String,
    pub(super) scope: RequestScope,
    pub(super) bank: String,
    pub(super) subject: String,
    pub(super) journal: PathBuf,
    #[expect(dead_code, reason = "the per-test directory, held for its drop")]
    journal_dir: FixtureDir,
    pub(super) salt: [u8; 16],
}

impl Attempt {
    pub(super) fn new() -> Self {
        let scope = subject_scope();
        let (bank, subject) = scope_parts(&scope);
        let journal_dir = FixtureDir::create(&unique("erase-journal"));
        Self {
            scratch: Scratch::new(),
            request_id: unique("req"),
            scope,
            bank,
            subject,
            journal: journal_dir.path().join("erasure-journal.json"),
            journal_dir,
            salt: rand::random::<[u8; 16]>(),
        }
    }

    pub(super) fn store_path(&self) -> PathBuf {
        self.scratch.0.join("store.db")
    }

    pub(super) fn ctx(&self) -> OpContext {
        OpContext::new(Bank::new(self.bank.clone()), Actor::Harness)
    }

    pub(super) fn args<'a>(
        &'a self,
        store: &'a dyn ComplianceStore,
        ctx: &'a OpContext,
        sink: &'a dyn EraseSink,
    ) -> EraseArgs<'a> {
        EraseArgs {
            store,
            journal_path: &self.journal,
            maint: None,
            ctx,
            subject_id: &self.subject,
            legal_basis: "GDPR_ART_17",
            request_id: &self.request_id,
            reason_code: None,
            confirm: true,
            salt: &self.salt,
            timestamp: NOW,
            sink,
        }
    }

    pub(super) fn env<'a>(
        &'a self,
        store: &'a dyn ComplianceStore,
        peers: &'a dyn clio_store::PeerActionStore,
        sink: &'a dyn EraseSink,
    ) -> crate::erasure_reconcile::ErasureRetryEnv<'a> {
        crate::erasure_reconcile::ErasureRetryEnv {
            store,
            peers,
            journal_path: &self.journal,
            maint: None,
            actor: Actor::Harness,
            legal_basis: "GDPR_ART_17",
            salt: &self.salt,
            timestamp: NOW,
            sink,
        }
    }
}

/// A persisted confirmation over an open file-backed store.
pub(super) fn persisted() -> (Attempt, SqliteStore) {
    let attempt = Attempt::new();
    let store = open_sqlite(&attempt.store_path());
    persist(&store, &started_record(&attempt.request_id, &attempt.scope));
    (attempt, store)
}

/// The live bank set one attempt's effect fans out to, read from its store.
pub(super) fn live_set(attempt: &Attempt, store: &SqliteStore) -> Vec<String> {
    store
        .erase_bank_set(&attempt.bank, &attempt.subject)
        .expect("live bank set")
}

/// Every event row one action id holds.
pub(super) fn action_rows(store: &SqliteStore, action_id: &str) -> Vec<LocalEventView> {
    store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect()
}

/// The durable action id one attempt's erasure is keyed by.
pub(super) fn action_id(attempt: &Attempt) -> String {
    erasure_action_id(&attempt.request_id)
}

/// The durable row a request id currently holds.
pub(super) fn load_row(store: &SqliteStore, request_id: &str) -> DurableRequestRow {
    store.load(request_id).expect("load").expect("row exists")
}

/// The committed tombstone count one request's erasure wrote.
pub(super) fn tombstone_count(attempt: &Attempt, store: &SqliteStore) -> usize {
    store
        .read_tombstones(&attempt.bank, &attempt.request_id)
        .expect("tombstones")
        .len()
}

/// A telemetry sink that lands a concurrent bank write before the settle.
struct RacingSink<'a> {
    durable: &'a dyn DurableRequestStore,
    row: DurableRequestRow,
}

impl std::fmt::Debug for RacingSink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RacingSink")
    }
}

impl EraseSink for RacingSink<'_> {
    fn emit(&self, _event: EraseEvent) {
        self.durable.save(&self.row).expect("the peer write lands");
    }
}

/// A writer that loses the intent race: the row lands, then the write fails as a duplicate.
struct RacingIntent<'a> {
    inner: &'a dyn LocalEventStore,
}

impl LocalEventStore for RacingIntent<'_> {
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

/// The refusal one `attempt` sees when its durable intent write fails.
fn intent_write_failure(attempt: &Attempt, store: &SqliteStore) -> AmError {
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(store, &ctx, &sink);
    let live = live_set(attempt, store);
    let events = FaultEvents::intent_fails();
    erase_confirmed_request(store, &events, store, &attempt.request_id, &args, &live)
        .expect_err("a failed intent must refuse")
}

/// A failed intent write refuses before any erasure effect.
#[test]
fn an_intent_write_failure_refuses_before_the_effect() {
    let (attempt, store) = persisted();
    let err = intent_write_failure(&attempt, &store);
    assert_eq!(err.code, ErrorCode::Internal, "the injected intent fault");
    assert_eq!(
        tombstone_count(&attempt, &store),
        0,
        "a refused intent runs no store effect"
    );
}

/// A failed intent write records no durable denial.
#[test]
fn an_intent_write_failure_writes_no_denial() {
    let (attempt, store) = persisted();
    let err = intent_write_failure(&attempt, &store);
    assert_eq!(err.code, ErrorCode::Internal, "the injected intent fault");
    assert!(!attempt.journal.exists(), "no journal denial is recorded");
}

/// A failed intent write leaves the subject's key intact.
#[test]
fn an_intent_write_failure_loses_no_key() {
    let (attempt, store) = persisted();
    let err = intent_write_failure(&attempt, &store);
    assert_eq!(err.code, ErrorCode::Internal, "the injected intent fault");
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "a refused intent loses no key"
    );
}

/// A same-id row of another event kind refuses instead of resuming.
#[test]
fn an_intent_id_held_by_another_event_kind_refuses() {
    let (attempt, store) = persisted();
    let id = action_id(&attempt);
    store
        .record_intent(&LocalEventIntent {
            action_id: &id,
            actor: "harness",
            scope: RESTORE_EVENT_KIND,
            kind: RESTORE_EVENT_KIND,
            event_time: NOW,
        })
        .expect("the foreign intent records");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect_err("a foreign-kind row must refuse the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    let rows = action_rows(&store, &id);
    assert_eq!(rows.len(), 1, "the foreign row stays the only row");
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert_eq!(
        rows[0].kind, RESTORE_EVENT_KIND,
        "the foreign row is never mutated"
    );
}

/// A concurrent caller that wins the intent write is resumed, not refused.
#[test]
fn a_racing_intent_write_resumes_instead_of_refusing() {
    let (attempt, store) = persisted();
    store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome = erase_confirmed_request(
        &store,
        &RacingIntent { inner: &store },
        &store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the raced intent resumes the flow");
    assert_eq!(outcome.status, "COMPLETED");
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
}

/// A same-scope repeat is idempotent: no second outcome and no store work.
#[test]
fn a_same_scope_repeat_is_idempotent() {
    let (attempt, store) = persisted();
    store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let first = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect("the first attempt completes");
    assert_eq!(first.status, "COMPLETED");
    let repeat = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect("a same-scope repeat is idempotent");
    assert_eq!(repeat.status, "COMPLETED");
    assert!(
        store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the key stays destroyed"
    );
    assert_eq!(
        action_rows(&store, &action_id(&attempt)).len(),
        1,
        "one allowed outcome, never a second"
    );
    // A re-run would add a subject-level tombstone; the count pins no re-run.
    assert_eq!(
        tombstone_count(&attempt, &store),
        1,
        "the repeat repeats no store work"
    );
}

/// A changed scope refuses before any new store effect.
#[test]
fn a_changed_scope_repeat_refuses_before_the_effect() {
    let (attempt, store) = persisted();
    store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect("the first attempt completes");
    let events_before = store.list_events().expect("events").len();
    let other_subject = unique("other-subject");
    let changed = EraseArgs {
        subject_id: &other_subject,
        ..attempt.args(&store, &ctx, &sink)
    };
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &changed, &live)
        .expect_err("a changed scope must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        store.list_events().expect("events").len(),
        events_before,
        "the refusal writes no new intent"
    );
}

/// A peer slot recorded mid-effect keeps the event pending as PROCESSING, even
/// though the peers confirmed before it already carry durable evidence: the
/// gate re-reads the record's slots, so the new slot alone keeps completion
/// open.
#[test]
fn a_peer_recorded_before_the_settle_keeps_completion_pending() {
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let mut started = started_record(&attempt.request_id, &attempt.scope);
    started
        .peers
        .insert(peer_bank.clone(), PeerState::Acknowledged);
    started.peer_banks.insert(peer_bank.clone());
    {
        let store = open_sqlite(&attempt.store_path());
        persist(&store, &started);
        store
            .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
            .expect("seed the subject item");
        // The confirmed peer's own durable acknowledgement, so it settles by
        // evidence rather than by a missing row; only the mid-effect slot is
        // left without one.
        begin_denial(&store, &attempt.request_id, &peer_bank).expect("the peer row begins");
        mark_acknowledged(&store, &attempt.request_id, &peer_bank, "deletion-9")
            .expect("the peer acknowledges");
    }
    let store = open_sqlite(&attempt.store_path());
    let mut joined = started.clone();
    joined
        .peers
        .insert(unique("joined-bank"), PeerState::Pending);
    let sink = RacingSink {
        durable: &store,
        row: record_to_row(&joined),
    };
    let ctx = attempt.ctx();
    let args = attempt.args(&store, &ctx, &sink);
    let live = vec![attempt.bank.clone(), peer_bank];
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a pending peer slot keeps completion pending"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id(&attempt)],
        "the event stays pending"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
}

// Declared here, beside the fixtures it reuses: the peer-evidence settle,
// peer-action reconcile, and prune-survival coverage.
#[path = "erasure_retry_peer_evidence_tests.rs"]
mod erasure_retry_peer_evidence_tests;
