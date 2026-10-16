// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twins of two remaining
//! confirmed-erasure fault points: a changed-scope retry that refuses before
//! any new effect, and a key-probe error that keeps the durable intent
//! pending.
//!
//! ## Owns
//! - The Postgres changed-scope refusal and key-probe-error assertions
//! - The Postgres key-probe-error store double
//!
//! ## Does not own
//! - The retry contract (`erasure_retry.rs`)
//! - The shared SQLite fault fixtures (`erasure_retry_tests.rs`,
//!   `erasure_retry_reconcile_tests.rs`)
//! - The private-schema Postgres fixture and its skip note
//!   (`subject_request_store_wiring_pg_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres retry stops refusing a changed scope or
//! stops keeping an intent pending through a key-probe error.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the Postgres changed-scope and key-probe-error
//! fault twins.

use crate::erase::{EraseArgs, EraseVecSink};
use crate::erasure_retry::{erase_confirmed_request, erasure_action_id};
use crate::subject_request_store_wiring::erasure_retry_tests::Attempt;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::erasure_retry_pg_tests::{
    pg_action_rows, pg_live_set,
};
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::pg_fixture_or_skip;
use crate::subject_request_store_wiring::subject_request_store_wiring_tests::{
    persist, started_record, wiring_item,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, ItemIdentity, LocalEventState, LocalEventStore,
    Store, SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{AmError, ErrorCode};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A compliance-store double whose key probe fails, so the retry sees a failed
/// probe and never a key state. It mirrors the SQLite `FaultStore` probe-error
/// fault while the durable and event stores stay the real Postgres store.
struct PgKeyProbeFault {
    erase_calls: AtomicUsize,
}

impl PgKeyProbeFault {
    fn new() -> Self {
        Self {
            erase_calls: AtomicUsize::new(0),
        }
    }
}

impl ComplianceStore for PgKeyProbeFault {
    fn list_subject_items(&self, _: &str, _: &str) -> Result<Vec<ItemIdentity>, AmError> {
        Ok(vec![])
    }

    fn erase_subject(&self, _: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
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
        Err(AmError::new(
            ErrorCode::Internal,
            "injected key probe failure",
        ))
    }

    fn read_tombstones(&self, _: &str, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }

    fn list_tombstones(&self, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }
}

/// A changed scope refuses before any new store effect on the real Postgres
/// store, and the refusal writes no new intent event.
#[test]
fn postgres_a_changed_scope_retry_refuses_before_the_effect() {
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
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the first attempt completes");
    let events_before = fixture.store.list_events().expect("events").len();
    let other_subject = unique("other-subject");
    let changed = EraseArgs {
        subject_id: &other_subject,
        ..attempt.args(&fixture.store, &ctx, &sink)
    };
    let err = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &changed,
        &live,
    )
    .expect_err("a changed scope must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        fixture.store.list_events().expect("events").len(),
        events_before,
        "the refusal writes no new intent"
    );
}

/// A key-probe error keeps the durable intent pending on the real Postgres
/// store and runs no effect, as the SQLite key-probe twin pins.
#[test]
fn postgres_a_key_probe_error_keeps_the_intent_pending() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    let double = PgKeyProbeFault::new();
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
    .expect_err("the key probe failure is reported");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        0,
        "no effect ran"
    );
    assert!(
        !attempt.journal.exists(),
        "a key probe error writes no denial"
    );
    let rows = pg_action_rows(&fixture.store, &erasure_action_id(&attempt.request_id));
    assert_eq!(rows.len(), 1, "the intent row stays the only row");
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
}
