// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the request facade's retry surface:
//! the pending erasure action id and effect counts one retry reports, and the
//! peer-denial reconcile.
//!
//! ## Owns
//! - The SQLite retry and peer-retry assertions
//!
//! ## Does not own
//! - The facade under test (`subject_request_service.rs`)
//! - The shared store, record, item, and retry fixtures
//!   (`subject_request_service_fixtures.rs`)
//! - The status-fold assertions (`subject_request_service_report_tests.rs`)
//! - The Postgres twin bodies (`subject_request_service_pg_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the retry's effect counts, its pending action id, or
//! the peer reconcile changes.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::EraseVecSink;
use crate::erasure_reconcile::ErasureRetryEnv;
use crate::erasure_retry::erasure_action_id;
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::inventory_fixture::FixtureDir;
use crate::subject_peer::{begin_denial, mark_acknowledged};
use crate::subject_request::{PeerState, RequestState};
use crate::subject_request_events::record_peer_denial_intent;
use crate::subject_request_service::{SubjectRequestEnv, peer_retry, report, retry_erasure};
use crate::subject_request_service_fixtures::{
    NOW, ctx, env, item, persist, record, retry_args, store,
};
use crate::test_unique::unique;
use clio_store::{ComplianceStore, DekProvider, LocalDevKms, LocalEventStore, SqliteStore, Store};
use clio_types::{Actor, ErrorCode};
use std::sync::Arc;

/// A failed outcome write leaves one pending action id visible; a retry over
/// the restarted store reconciles it and repeats no protected effect.
#[test]
fn retry_names_the_pending_action_and_repeats_no_protected_effect() {
    let request_id = unique("req-retry");
    let dir = FixtureDir::create(&unique("svc-retry"));
    let path = dir.path().join("store.db");
    let journal = dir.path().join("erasure-journal.json");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let ctx = ctx("bank-a");
    let salt = [0u8; 16];
    let sink = EraseVecSink::default();
    {
        let store = Arc::new(SqliteStore::open(&path, Arc::clone(&kms)).expect("open"));
        store
            .create_memory_item(&item("bank-a", "subject-1"), "tester")
            .expect("seed");
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
        let env = SubjectRequestEnv {
            durable: &*store,
            events: &events,
            peers: &*store,
            copies: &*store,
        };
        let err = retry_erasure(&env, &ctx, &request_id, &args, &["bank-a".to_owned()])
            .expect_err("the failed outcome write is reported");
        assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
        assert_eq!(
            store.pending_actions().expect("pending"),
            vec![erasure_action_id(&request_id)],
            "one pending action id stays visible"
        );
    }
    // The command process restarts: reopen the same store over the same KMS.
    let store = SqliteStore::open(&path, Arc::clone(&kms)).expect("reopen");
    let before = store
        .read_tombstones("bank-a", &request_id)
        .expect("tombstones")
        .len();
    assert!(before > 0, "the first attempt committed the effect");

    // The read surface names the pending action, not only the event review.
    let restarted = report(&env(&store), &ctx, &request_id).expect("the status reads");
    assert!(
        restarted
            .pending_operator_actions
            .iter()
            .any(|action| action.action_id == erasure_action_id(&request_id)),
        "the status names the pending action: {restarted:?}"
    );
    let args = retry_args(
        &store,
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

/// A peer retry reconciles a pending peer-denial row from its acknowledged
/// evidence without running an effect.
#[test]
fn peer_retry_reconciles_a_pending_peer_denial_row() {
    let journal = std::env::temp_dir().join(unique("svc-peer-journal"));
    let store = store();
    let request_id = unique("req-peer");
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
