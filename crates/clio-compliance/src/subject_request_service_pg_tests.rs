// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twins of the facade retry
//! tests: a failed outcome leaves one pending erasure action id that a retry
//! reconciles without repeating the protected effect, and a peer retry
//! reconciles a pending peer-denial row from its acknowledged evidence.
//!
//! ## Owns
//! - The private-schema Postgres fixture these tests open
//! - The Postgres retry and peer-retry assertions
//!
//! ## Does not own
//! - The facade under test (`subject_request_service.rs`)
//! - The shared retry fixtures (`subject_request_service_fixtures.rs`, reused)
//! - The SQLite twins (`subject_request_service_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres backend stops leaving a pending action
//! after a failed outcome, repeats a protected effect on retry, or stops
//! reconciling a peer-denial row from its evidence.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::EraseVecSink;
use crate::erasure_reconcile::ErasureRetryEnv;
use crate::erasure_retry::erasure_action_id;
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::inventory_fixture::FixtureDir;
use crate::inventory_pg_support_tests::{pg_checked_url, pg_skip_note};
use crate::subject_peer::{begin_denial, mark_acknowledged};
use crate::subject_request::{PeerState, RequestState};
use crate::subject_request_events::record_peer_denial_intent;
use crate::subject_request_service::{SubjectRequestEnv, peer_retry, report, retry_erasure};
use crate::subject_request_service_fixtures::{NOW, ctx, item, persist, record, retry_args};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, DekProvider, LocalDevKms, LocalEventStore, PgTestGuard, PostgresStore,
    ScopedSchema, Store, lock_pg_test, probe_postgres,
};
use clio_types::{Actor, AmError, ErrorCode};
use std::sync::Arc;

/// The suite name every skip note in this module carries.
const SUITE: &str = "subject request service Postgres tests";

/// A private-schema Postgres store, held with the schema and lock that own it.
struct PgFixture {
    store: PostgresStore,
    _schema: ScopedSchema,
    _guard: PgTestGuard,
}

/// True only when the operator explicitly opted out of Postgres-backed tests.
fn skip_requested() -> bool {
    matches!(
        std::env::var("CLIO_SKIP_PG_TESTS").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

/// Open the live Postgres fixture. An unreachable Postgres panics unless the
/// operator set `CLIO_SKIP_PG_TESTS`, which prints the skip note and returns
/// `None`. Neither the note nor the panic carries a connection string.
fn pg_fixture_or_skip() -> Option<PgFixture> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    if let Err(detail) = probe_postgres(&base) {
        let err = AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}"));
        if skip_requested() {
            eprintln!("{}", pg_skip_note(SUITE, &err));
            return None;
        }
        panic!("{SUITE} require a reachable Postgres: {err}");
    }
    let schema = ScopedSchema::new(&base);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(schema.url(), kms).expect("open the scoped fixture store");
    Some(PgFixture {
        store,
        _schema: schema,
        _guard: guard,
    })
}

/// A failed outcome write leaves one pending action id visible; a retry over
/// the same store reconciles it and repeats no protected effect.
#[test]
fn postgres_retry_names_the_pending_action_and_repeats_no_protected_effect() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let dir = FixtureDir::create(&unique("svc-pg-retry"));
    let journal = dir.path().join("erasure-journal.json");
    let store = Arc::new(fixture.store);
    let request_id = unique("req-pg-retry");
    let ctx = ctx("bank-a");
    let salt = [0u8; 16];
    let sink = EraseVecSink::default();
    store
        .create_memory_item(&item("bank-a", "subject-1"), "tester")
        .expect("seed the subject item");
    persist(&*store, &record(&request_id, RequestState::InProgress));
    let args = retry_args(
        &*store,
        &ctx,
        &sink,
        &journal,
        &request_id,
        "subject-1",
        &salt,
    );
    let events = FaultEvents::outcome_fails_over(&*store);
    let first_env = SubjectRequestEnv {
        durable: &*store,
        events: &events,
        peers: &*store,
        copies: &*store,
    };
    let err = retry_erasure(&first_env, &ctx, &request_id, &args, &["bank-a".to_owned()])
        .expect_err("the failed outcome write is reported");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![erasure_action_id(&request_id)],
        "one pending action id stays visible"
    );
    let before = store
        .read_tombstones("bank-a", &request_id)
        .expect("tombstones")
        .len();
    assert!(before > 0, "the first attempt committed the effect");
    let status = report(&env(&store), &ctx, &request_id).expect("the status reads");
    assert!(
        status
            .pending_operator_actions
            .iter()
            .any(|action| action.action_id == erasure_action_id(&request_id)),
        "the status names the pending action: {status:?}"
    );
    let args = retry_args(
        &*store,
        &ctx,
        &sink,
        &journal,
        &request_id,
        "subject-1",
        &salt,
    );
    let retried = retry_erasure(
        &env(&store),
        &ctx,
        &request_id,
        &args,
        &["bank-a".to_owned()],
    )
    .expect("the retry reconciles");
    assert_eq!(
        retried.action_id.as_deref(),
        Some(erasure_action_id(&request_id).as_str())
    );
    assert_eq!(retried.effects_before, before);
    assert_eq!(
        retried.effects_after, before,
        "the retry repeats no protected effect"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the retry settles the pending action"
    );
    assert!(
        !retried
            .report
            .pending_operator_actions
            .iter()
            .any(|action| action.action_id == erasure_action_id(&request_id)),
        "the retry clears the pending action from the status"
    );
}

/// The facade environment over one Postgres store handle.
fn env(store: &PostgresStore) -> SubjectRequestEnv<'_> {
    SubjectRequestEnv {
        durable: store,
        events: store,
        peers: store,
        copies: store,
    }
}

/// A peer retry reconciles a pending peer-denial row from its acknowledged
/// evidence without running an effect.
#[test]
fn postgres_peer_retry_reconciles_a_pending_peer_denial_row() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let journal = std::env::temp_dir().join(unique("svc-pg-peer-journal"));
    let store = fixture.store;
    let request_id = unique("req-pg-peer");
    let peer_id = unique("peer-bank");
    let mut source = record(&request_id, RequestState::InProgress);
    source.peers.insert(peer_id.clone(), PeerState::Pending);
    source.peer_banks.insert(peer_id.clone());
    persist(&store, &source);
    begin_denial(&store, &request_id, &peer_id).expect("the peer row begins");
    record_peer_denial_intent(&store, &request_id, &peer_id, Actor::Harness, NOW)
        .expect("the peer intent is durable");
    mark_acknowledged(&store, &request_id, &peer_id, "deletion-9").expect("the peer acknowledges");
    let salt = [0u8; 16];
    let sink = EraseVecSink::default();
    let retry_env = ErasureRetryEnv {
        store: &store,
        peers: &store,
        journal_path: &journal,
        maint: None,
        actor: Actor::Harness,
        legal_basis: "GDPR_ART_17",
        salt: &salt,
        timestamp: NOW,
        sink: &sink,
    };
    let report = peer_retry(&env(&store), &ctx("bank-a"), &request_id, &retry_env)
        .expect("the peer retry reconciles");
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the peer action is settled"
    );
    assert_eq!(report.peers.get(&peer_id), Some(&PeerState::Acknowledged));
}
