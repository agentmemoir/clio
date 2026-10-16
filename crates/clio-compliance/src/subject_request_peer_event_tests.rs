// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer-denial producer coverage against
//! the durable local event store: the intent write lands before delivery and
//! resumes without a second row, the outcome writes only from the peer's
//! durable acknowledgement or refusal row, the envelope carries the pending
//! result before the settle and the done or refused outcome after it, and no
//! stage carries subject prose, key material, or a proof reference.
//!
//! ## Owns
//! - The durable peer-denial intent, outcome, and envelope assertions
//!
//! ## Does not own
//! - The envelope contract (`subject_request_events.rs`) and the producer
//!   (`subject_request_peer_events.rs`)
//! - The durable per-peer row contract and its SQL (`clio-store`)
//! - The scratch SQLite fixture (`subject_peer_fixtures.rs`, shared with
//!   `subject_peer_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a peer outcome is written before the durable
//! acknowledgement, when the intent is not restart-safe, or when the envelope
//! publishes evidence before the row settles.
//!
//! Test-only. Never compiled into production builds.

use crate::subject_peer::subject_peer_fixtures::{Scratch, open_sqlite};
use crate::subject_peer::{
    begin_denial, mark_acknowledged, mark_delivered, mark_refused, peer_action_id,
    record_disposition,
};
use crate::subject_request::RequestScope;
use crate::subject_request_events::{
    ActionEventKind, ActionOutcome, PendingResult, ProtectedAction, peer_denial_event,
    record_peer_denial_intent, record_peer_denial_outcome,
};
use crate::test_unique::unique;
use clio_store::{
    LocalEventState, LocalEventStore, PeerActionDisposition, PeerActionKind, PeerActionStore,
    SqliteStore,
};
use clio_types::{Actor, ErrorCode};

const NOW: &str = "2026-01-01T00:00:00Z";

/// One open scratch store, held with the directory guard that removes it on
/// drop, so the store's file outlives the test's use of it.
fn scratch_store() -> (Scratch, SqliteStore) {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    (scratch, store)
}

/// The peer-denial envelope of one stage, content-limited to the derived
/// action id, the scope the caller holds, and the stage's pending or outcome
/// payload.
fn scope(request_id: &str) -> RequestScope {
    RequestScope::Subject {
        bank_id: "event-bank".into(),
        subject_id: format!("subject-of-{request_id}"),
    }
}

/// The intent is durable before delivery, resumes without a second row, and
/// its envelope carries only the pending result.
#[test]
fn a_peer_denial_intent_is_durable_before_delivery_and_resumes() {
    let (_scratch, store) = scratch_store();
    let request_id = unique("req");
    let peer_id = unique("peer");
    record_peer_denial_intent(&store, &request_id, &peer_id, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    record_peer_denial_intent(&store, &request_id, &peer_id, Actor::Harness, NOW)
        .expect("a repeated intent resumes");
    let action = peer_action_id(&request_id, &peer_id);
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|row| row.action_id == action)
        .collect();
    assert_eq!(rows.len(), 1, "one durable intent, never a second");
    assert_eq!(rows[0].kind, "peer_denial", "the producer's own kind");
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no result before the settle");
    let event = peer_denial_event(
        &request_id,
        &peer_id,
        &scope(&request_id),
        None,
        Actor::Harness,
        NOW,
        1,
    )
    .expect("the intent envelope");
    assert_eq!(event.payload.producer, ProtectedAction::PeerDenial);
    assert_eq!(event.payload.kind, ActionEventKind::Intent);
    assert_eq!(
        event.payload.pending,
        Some(PendingResult::PeerDenialPending)
    );
    assert!(
        event.payload.outcome.is_none(),
        "no outcome before delivery"
    );
    assert_eq!(event.correlation_id, request_id, "the request is the scope");
    assert_eq!(event.payload.intent.action_id, action, "the derived key");
}

/// The outcome writes only from the peer's durable acknowledgement or refusal
/// row, and the envelope publishes the matching outcome stage.
#[test]
fn a_peer_outcome_needs_the_durable_acknowledgement_or_refusal() {
    let (_scratch, store) = scratch_store();
    let request_id = unique("req");
    let acknowledged = unique("peer-ack");
    let refused = unique("peer-refused");
    let err = record_peer_denial_outcome(&store, &store, &request_id, &acknowledged)
        .expect_err("no durable row supports no outcome");
    assert_eq!(err.code, ErrorCode::NotFound);
    record_peer_denial_intent(&store, &request_id, &acknowledged, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    begin_denial(&store, &request_id, &acknowledged).expect("the peer row begins");
    mark_delivered(&store, &request_id, &acknowledged).expect("the delivery records");
    let err = record_peer_denial_outcome(&store, &store, &request_id, &acknowledged)
        .expect_err("a delivered row is not an acknowledgement");
    assert_eq!(err.code, ErrorCode::Forbidden);
    mark_acknowledged(&store, &request_id, &acknowledged, "deletion-9")
        .expect("the acknowledgement records");
    record_peer_denial_outcome(&store, &store, &request_id, &acknowledged)
        .expect("the durable acknowledgement settles the outcome");
    let action = peer_action_id(&request_id, &acknowledged);
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|row| row.action_id == action)
        .collect();
    assert_eq!(rows.len(), 1, "one settled outcome row");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some("peer denial acknowledged"));
    let row = PeerActionStore::load(&store, &request_id, &acknowledged, PeerActionKind::Denial)
        .expect("load the peer row")
        .expect("the durable row exists");
    let event = peer_denial_event(
        &request_id,
        &acknowledged,
        &scope(&request_id),
        Some(&row),
        Actor::Harness,
        NOW,
        2,
    )
    .expect("the outcome envelope");
    assert_eq!(event.payload.kind, ActionEventKind::Outcome);
    assert_eq!(event.payload.outcome, Some(ActionOutcome::Done));
    assert!(event.payload.pending.is_none(), "the peer owes nothing");
    record_peer_denial_intent(&store, &request_id, &refused, Actor::Harness, NOW)
        .expect("the refused peer's intent is durable before delivery");
    begin_denial(&store, &request_id, &refused).expect("the refused row begins");
    mark_refused(&store, &request_id, &refused).expect("the refusal records");
    record_peer_denial_outcome(&store, &store, &request_id, &refused)
        .expect("the durable refusal settles the outcome");
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|row| row.action_id == peer_action_id(&request_id, &refused))
        .collect();
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(rows[0].result.as_deref(), Some("peer denial refused"));
    let row = PeerActionStore::load(&store, &request_id, &refused, PeerActionKind::Denial)
        .expect("load the refused row")
        .expect("the durable row exists");
    let event = peer_denial_event(
        &request_id,
        &refused,
        &scope(&request_id),
        Some(&row),
        Actor::Harness,
        NOW,
        3,
    )
    .expect("the refused outcome envelope");
    assert_eq!(
        event.payload.outcome,
        Some(ActionOutcome::Refused {
            reason: "peer denial refused".into(),
        })
    );
    assert!(event.payload.pending.is_none(), "the peer owes nothing");
}

/// A refused peer row that then recorded an approved retention publishes the
/// retention's own content-free result, never a denial.
#[test]
fn a_retention_approved_peer_publishes_its_own_result_not_a_denial() {
    let (_scratch, store) = scratch_store();
    let request_id = unique("req");
    let peer_id = unique("peer-retained");
    record_peer_denial_intent(&store, &request_id, &peer_id, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    begin_denial(&store, &request_id, &peer_id).expect("the peer row begins");
    mark_refused(&store, &request_id, &peer_id).expect("the refusal records");
    record_disposition(
        &store,
        &request_id,
        &peer_id,
        PeerActionDisposition::RetentionApproved,
        Some("hold-7"),
    )
    .expect("the approved retention records");
    record_peer_denial_outcome(&store, &store, &request_id, &peer_id)
        .expect("the recorded retention settles the outcome");
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|row| row.action_id == peer_action_id(&request_id, &peer_id))
        .collect();
    assert_eq!(rows.len(), 1, "one settled outcome row");
    assert_eq!(
        rows[0].state,
        LocalEventState::Allowed,
        "an approved retention is not a denial"
    );
    assert_eq!(
        rows[0].result.as_deref(),
        Some("peer retention approved"),
        "the content-free retention result"
    );
    let row = PeerActionStore::load(&store, &request_id, &peer_id, PeerActionKind::Denial)
        .expect("load the peer row")
        .expect("the durable row exists");
    let event = peer_denial_event(
        &request_id,
        &peer_id,
        &scope(&request_id),
        Some(&row),
        Actor::Harness,
        NOW,
        4,
    )
    .expect("the retained outcome envelope");
    assert_eq!(
        event.payload.outcome,
        Some(ActionOutcome::Done),
        "a resolved row is done, never refused"
    );
    assert!(event.payload.pending.is_none(), "the peer owes nothing");
}

/// A refused peer row that then recorded the peer's removal evidence
/// publishes the recorded result, never a denial.
#[test]
fn a_recorded_removal_evidence_publishes_its_own_result_not_a_denial() {
    let (_scratch, store) = scratch_store();
    let request_id = unique("req");
    let peer_id = unique("peer-removed");
    record_peer_denial_intent(&store, &request_id, &peer_id, Actor::Harness, NOW)
        .expect("the intent is durable before delivery");
    begin_denial(&store, &request_id, &peer_id).expect("the peer row begins");
    mark_refused(&store, &request_id, &peer_id).expect("the refusal records");
    record_disposition(
        &store,
        &request_id,
        &peer_id,
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("the removal evidence records");
    record_peer_denial_outcome(&store, &store, &request_id, &peer_id)
        .expect("the recorded removal evidence settles the outcome");
    let rows: Vec<_> = store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|row| row.action_id == peer_action_id(&request_id, &peer_id))
        .collect();
    assert_eq!(rows.len(), 1, "one settled outcome row");
    assert_eq!(
        rows[0].state,
        LocalEventState::Allowed,
        "an evidenced removal is not a denial"
    );
    assert_eq!(
        rows[0].result.as_deref(),
        Some("peer removal evidence recorded"),
        "the content-free removal result"
    );
    assert!(
        rows[0].result.as_deref().is_some_and(|result| {
            !result.contains("deletion-9") && !result.contains(&request_id)
        }),
        "the proof reference and the request never enter the result text"
    );
    let row = PeerActionStore::load(&store, &request_id, &peer_id, PeerActionKind::Denial)
        .expect("load the peer row")
        .expect("the durable row exists");
    let event = peer_denial_event(
        &request_id,
        &peer_id,
        &scope(&request_id),
        Some(&row),
        Actor::Harness,
        NOW,
        5,
    )
    .expect("the removal outcome envelope");
    assert_eq!(event.payload.outcome, Some(ActionOutcome::Done));
    assert!(event.payload.pending.is_none(), "the peer owes nothing");
}
