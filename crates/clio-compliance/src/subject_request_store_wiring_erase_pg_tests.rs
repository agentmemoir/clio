// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the durable
//! erasure seam: a subject-bank write landing after confirmation cannot widen
//! the erasure, a peer recorded before the settle keeps completion pending
//! until it is acknowledged, and a partial erasure retry never reports
//! completion early.
//!
//! ## Owns
//! - The Postgres durable-erasure seam assertions and the racing settle sink
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The shared SQLite seam tests and fixtures
//!   (`subject_request_store_wiring_erase_tests.rs`,
//!   `subject_request_store_wiring_tests.rs`)
//! - The Postgres retry helpers (`erasure_retry_pg_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Tests here break when the durable Postgres seam stops refusing before any
//! effect or reports a completion the store never reached.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::{EraseEvent, EraseSink, EraseVecSink};
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::subject_request::PeerState;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::erasure_retry_pg_tests::{
    pg_action_rows, pg_live_set, pg_tombstone_count,
};
use crate::subject_request_store_wiring::erasure_retry_tests::Attempt;
use crate::subject_request_store_wiring::subject_request_store_wiring_pg_tests::{
    PgFixture, pg_fixture_or_skip,
};
use crate::subject_request_store_wiring::subject_request_store_wiring_tests::{
    persist, started_record, wiring_item,
};
use crate::subject_request_store_wiring::{erase_from_durable_store, record_to_row};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, DurableRequestRow, DurableRequestStore, ItemIdentity,
    LocalEventState, LocalEventStore, PostgresStore, Store, SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{AmError, ErrorCode};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A sink that lands a concurrent request write before the settle, as a peer
/// joining mid-effect does.
struct PgRacingSink<'a> {
    durable: &'a dyn DurableRequestStore,
    row: DurableRequestRow,
}

impl std::fmt::Debug for PgRacingSink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgRacingSink")
    }
}

impl EraseSink for PgRacingSink<'_> {
    fn emit(&self, _event: EraseEvent) {
        self.durable.save(&self.row).expect("the peer write lands");
    }
}

/// A subject-bank write landing after the confirmation widens the live bank
/// set but can never widen the confirmed erasure: the seam refuses before any
/// intent event, journal denial, key loss, or store effect.
#[test]
fn postgres_a_new_subject_bank_write_after_confirmation_cannot_widen_the_erasure() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    let new_bank = unique("new-bank");
    let original = wiring_item(&attempt.bank, &attempt.subject);
    let new_item = wiring_item(&new_bank, &attempt.subject);
    fixture
        .store
        .create_memory_item(&original, "tester")
        .expect("seed the confirmed bank");
    persist(
        &fixture.store,
        &started_record(&attempt.request_id, &attempt.scope),
    );
    fixture
        .store
        .create_memory_item(&new_item, "tester")
        .expect("the new subject-bank write lands");
    let before = fixture
        .store
        .load(&attempt.request_id)
        .expect("load")
        .expect("row exists");
    let live = pg_live_set(&attempt, &fixture.store);
    assert_eq!(live.len(), 2, "the new write widens the live set: {live:?}");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let err = erase_from_durable_store(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect_err("a widened bank set must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        fixture.store.list_events().expect("events").is_empty(),
        "the refusal must precede any intent event"
    );
    assert!(!attempt.journal.exists(), "the refusal precedes any denial");
    assert!(
        !fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .expect("key"),
        "the refusal loses no key"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        0,
        "the refusal writes no tombstone"
    );
    assert_eq!(
        fixture
            .store
            .load(&attempt.request_id)
            .expect("load")
            .expect("row exists"),
        before,
        "the refusal must leave the durable row untouched"
    );
    assert!(
        fixture
            .store
            .get_memory_item(&attempt.bank, &original.id)
            .expect("readable")
            .is_some(),
        "the confirmed bank's item survives untouched"
    );
    assert!(
        fixture
            .store
            .get_memory_item(&new_bank, &new_item.id)
            .expect("readable")
            .is_some(),
        "the newly written bank's item survives untouched"
    );
}

/// A peer slot recorded before the settle keeps the effect pending as
/// PROCESSING, and acknowledging it lets a retry settle the same action.
#[test]
fn postgres_a_peer_recorded_before_the_settle_keeps_completion_pending() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let attempt = Attempt::new();
    let peer_bank = unique("peer-bank");
    let mut started = started_record(&attempt.request_id, &attempt.scope);
    started
        .peers
        .insert(peer_bank.clone(), PeerState::Acknowledged);
    started.peer_banks.insert(peer_bank.clone());
    persist(&fixture.store, &started);
    fixture
        .store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let joined_bank = unique("joined-bank");
    let mut joined = started.clone();
    joined.peers.insert(joined_bank.clone(), PeerState::Pending);
    joined.peer_banks.insert(joined_bank.clone());
    let sink = PgRacingSink {
        durable: &fixture.store,
        row: record_to_row(&joined),
    };
    let ctx = attempt.ctx();
    let args = attempt.args(&fixture.store, &ctx, &sink);
    let live = vec![attempt.bank.clone(), peer_bank.clone()];
    let outcome = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "a pending peer slot keeps completion pending"
    );
    let action_id = erasure_action_id(&attempt.request_id);
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![action_id.clone()],
        "the event stays pending"
    );
    let rows = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
    let tombstones = pg_tombstone_count(&attempt, &fixture.store);
    assert!(
        tombstones > 0,
        "the effect committed while the peer stayed pending"
    );
    // Acknowledging the joined peer durably and in the record lets the same
    // action settle, with no repeated destructive work.
    let mut acknowledged = joined.clone();
    acknowledged
        .peers
        .insert(joined_bank.clone(), PeerState::Acknowledged);
    persist(&fixture.store, &acknowledged);
    for peer in [&peer_bank, &joined_bank] {
        crate::subject_peer::begin_denial(&fixture.store, &attempt.request_id, peer)
            .expect("the peer row begins");
        crate::subject_peer::mark_acknowledged(
            &fixture.store,
            &attempt.request_id,
            peer,
            "deletion-9",
        )
        .expect("the peer acknowledges");
    }
    let live_all = vec![attempt.bank.clone(), peer_bank, joined_bank];
    let retried = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live_all,
    )
    .expect("the acknowledged retry settles");
    assert_eq!(retried.status, "COMPLETED");
    let settled = pg_action_rows(&fixture.store, &action_id);
    assert_eq!(settled[0].state, LocalEventState::Allowed);
    assert_eq!(settled[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        tombstones,
        "the settle repeats no destructive work"
    );
}

/// A thin [`ComplianceStore`] over the real Postgres store that counts erase
/// calls, so this suite drives the effect through the live store and still
/// pins one effect run per attempt.
struct PgCountingErase<'a> {
    inner: &'a PostgresStore,
    erase_calls: AtomicUsize,
}

impl ComplianceStore for PgCountingErase<'_> {
    fn list_subject_items(&self, bank: &str, subject: &str) -> Result<Vec<ItemIdentity>, AmError> {
        self.inner.list_subject_items(bank, subject)
    }

    fn erase_subject(&self, spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.erase_subject(spec)
    }

    fn subject_dek_destroyed(&self, subject: &str) -> Result<bool, AmError> {
        self.inner.subject_dek_destroyed(subject)
    }

    fn read_tombstones(
        &self,
        bank: &str,
        request_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        self.inner.read_tombstones(bank, request_id)
    }

    fn list_tombstones(&self, bank: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        self.inner.list_tombstones(bank)
    }
}

/// Install a `BEFORE INSERT` trigger that aborts every tombstone write, so the
/// Postgres erasure fails after it destroys the key and before it commits.
fn install_tombstone_trigger(fixture: &PgFixture) {
    fixture
        .execute(
            "CREATE FUNCTION fail_erase_tombstone() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'forced tombstone failure'; END $$",
        )
        .expect("create the fault function");
    fixture
        .execute(
            "CREATE TRIGGER fail_erase_tombstone BEFORE INSERT ON compliance_tombstones
             FOR EACH ROW EXECUTE FUNCTION fail_erase_tombstone()",
        )
        .expect("create the fault trigger");
}

/// Remove the injected tombstone trigger so a later retry can commit.
fn drop_tombstone_trigger(fixture: &PgFixture) {
    fixture
        .execute("DROP TRIGGER IF EXISTS fail_erase_tombstone ON compliance_tombstones")
        .expect("drop the fault trigger");
    fixture
        .execute("DROP FUNCTION IF EXISTS fail_erase_tombstone()")
        .expect("drop the fault function");
}

/// A partial erasure retried through the real Postgres store stays `PROCESSING`
/// while an injected in-transaction fault keeps the purge uncommitted, and
/// reports `COMPLETED` only once the purge commits.
#[test]
fn postgres_a_partial_erasure_retry_never_reports_completion_early() {
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
        .expect("seed the subject item the real erase destroys");
    let before = fixture
        .store
        .load(&attempt.request_id)
        .expect("load")
        .expect("row exists");
    // The fault aborts every tombstone insert inside the erasure transaction,
    // so the key is destroyed and the purge rolls back.
    install_tombstone_trigger(&fixture);
    let double = PgCountingErase {
        inner: &fixture.store,
        erase_calls: AtomicUsize::new(0),
    };
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&double, &ctx, &sink);
    let live = pg_live_set(&attempt, &fixture.store);
    let first = erase_from_durable_store(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("a partial failure reports PROCESSING");
    assert_eq!(first.status, "PROCESSING");
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        1,
        "the first attempt runs the effect once"
    );
    assert!(
        fixture
            .store
            .subject_dek_destroyed(&attempt.subject)
            .expect("key"),
        "the fault destroys the key before the purge rolls back"
    );
    assert_eq!(
        pg_tombstone_count(&attempt, &fixture.store),
        0,
        "the aborted purge commits no tombstone"
    );
    let retry = erase_from_durable_store(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the retried attempt also reports PROCESSING");
    assert_eq!(
        retry.status, "PROCESSING",
        "a retry never reports false completion"
    );
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        2,
        "each attempt runs the effect once"
    );
    assert_eq!(
        fixture
            .store
            .load(&attempt.request_id)
            .expect("load")
            .expect("row exists"),
        before,
        "an attempt must not rewrite the durable row"
    );
    // Remove the fault: the retry commits the purge and completes.
    drop_tombstone_trigger(&fixture);
    let recovered = erase_from_durable_store(
        &fixture.store,
        &fixture.store,
        &fixture.store,
        &attempt.request_id,
        &args,
        &live,
    )
    .expect("the recovered attempt completes");
    assert_eq!(recovered.status, "COMPLETED");
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        3,
        "each attempt runs the effect once"
    );
    assert!(
        pg_tombstone_count(&attempt, &fixture.store) > 0,
        "the recovered attempt commits the purge"
    );
}
