// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gated Postgres end-to-end evidence for
//! the restart and bank-scope arms of the durable subject-request tools: a
//! failed event outcome retried after a reopen, a partial bank scope refused
//! before effects, and an authorized scope with an offline peer that runs but
//! stays pending.
//!
//! ## Owns
//! - The Postgres restart, partial-scope, and offline-peer assertions
//!
//! ## Does not own
//! - The request sequencing or state machine (`clio-compliance`)
//! - The shared private-schema fixture (`subject_request_pg_support`)
//! - The lifecycle and refusal Postgres twins
//!   (`subject_request_tools_postgres_test`)
//!
//! ## Boundary
//!
//! Changes to the Postgres restart or bank-scope evidence should normally
//! require changes here. Changes to the request state machine or the tool
//! decoding should not.
//!
//! Keep this module focused on the live-backend restart and scope evidence.

use std::sync::Arc;

use clio_compliance::RequestState;
use serde_json::json;

#[path = "subject_request_pg_support/mod.rs"]
mod subject_request_pg_support;

use subject_request_pg_support::{
    BANK, FailingOutcomeEvents, PEER_BANK, PgFixture, SUBJECT, add_pending_peer, begin_peer_row,
    call, load_record, store_item,
};

/// A failed outcome write leaves one pending action; a retry over a reopened
/// state names it and reconciles it without repeating the protected effect.
#[test]
fn subject_request_retry_after_a_failed_outcome_reconciles_on_postgres() {
    let Some(fixture) = PgFixture::open() else {
        return;
    };
    let request = "req-pg-restart";
    let pending;
    {
        let base = &fixture.state;
        store_item(base, "itm-pg-restart", SUBJECT, BANK);
        let started = call(
            base,
            "subject_request_start",
            json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
        );
        assert_eq!(started["ok"], json!(true), "{started}");
        let failing = FailingOutcomeEvents::over(base);
        let mut first = base.as_ref().clone();
        first.events = failing;
        let first = Arc::new(first);
        let failed = call(
            &first,
            "subject_request_retry",
            json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
                   "legal_basis": "GDPR_ART_17", "confirm": true}),
        );
        assert_eq!(failed["ok"], json!(false), "{failed}");
        assert_eq!(failed["code"], json!("internal"), "{failed}");
        pending = first.events.pending_actions().expect("pending");
        assert_eq!(pending.len(), 1, "one pending action survives: {pending:?}");
    }

    let reopened = fixture.reopen();
    let pending_id = pending[0].clone();
    let status = call(
        &reopened,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    let pending_actions = status["pending_operator_actions"]
        .as_array()
        .expect("pending operator actions");
    assert!(
        pending_actions
            .iter()
            .any(|action| action["action_id"] == json!(pending_id)),
        "the status names the pending action: {status}"
    );
    let events = call(
        &reopened,
        "subject_request_events",
        json!({"bank": BANK, "request_id": request}),
    );
    let rows = events["rows"].as_array().expect("rows");
    assert!(
        rows.iter()
            .any(|row| row["action_id"] == json!(pending_id) && row["state"] == json!("pending")),
        "the reopened state names the pending action: {events}"
    );
    let retried = call(
        &reopened,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
    assert_eq!(retried["action_id"], json!(pending_id), "{retried}");
    assert_eq!(
        retried["effects_before"], retried["effects_after"],
        "the retry repeats no protected effect: {retried}"
    );
    assert!(
        reopened
            .events
            .pending_actions()
            .expect("pending")
            .is_empty(),
        "the retry settles the pending action"
    );
}

/// A partial bank scope refuses before effects; an authorized scope with an
/// offline peer runs but stays pending until the peer is evidenced.
#[test]
fn subject_request_bank_scope_arms_on_postgres() {
    let Some(fixture) = PgFixture::open() else {
        return;
    };
    let state = &fixture.state;

    let partial = "req-pg-partial";
    store_item(state, "itm-pg-partial-a", SUBJECT, BANK);
    store_item(state, "itm-pg-partial-b", SUBJECT, PEER_BANK);
    let started = call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": partial}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    let refused = call(
        state,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": partial, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("forbidden"), "{refused}");
    assert!(
        !state.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "a refused partial-bank erasure destroys no key"
    );
    assert_eq!(load_record(state, partial).state, RequestState::InProgress);

    let request = "req-pg-offline-peer";
    let peer = PEER_BANK;
    let started = call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    add_pending_peer(state, request, peer);
    begin_peer_row(state, request, peer);
    let ran = call(
        state,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(ran["ok"], json!(true), "{ran}");
    assert!(ran["action_id"].is_null(), "{ran}");
    assert!(
        ran["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the authorized scope runs the effect: {ran}"
    );
    let peers = call(
        state,
        "subject_request_peer_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(
        peers[peer],
        json!("pending"),
        "the offline peer stays pending: {peers}"
    );
    let reconciled = call(
        state,
        "subject_request_peer_retry",
        json!({"bank": BANK, "request_id": request, "legal_basis": "GDPR_ART_17"}),
    );
    assert_eq!(reconciled["ok"], json!(true), "{reconciled}");
    assert_eq!(reconciled["request_id"], json!(request));
    let disposed = call(
        state,
        "subject_request_peer_disposition",
        json!({"bank": BANK, "request_id": request, "peer_id": peer,
               "disposition": "removal_evidence", "proof_ref": "deletion-9"}),
    );
    assert_eq!(disposed["ok"], json!(true), "{disposed}");
    let retried = call(
        state,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
    assert!(retried["action_id"].is_string(), "{retried}");
    assert_eq!(
        retried["effects_before"], retried["effects_after"],
        "the retry repeats no protected effect: {retried}"
    );
}
