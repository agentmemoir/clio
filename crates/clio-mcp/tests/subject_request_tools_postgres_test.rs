// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gated Postgres end-to-end evidence that
//! the durable subject-request tools agree on one request and refuse unsafe
//! effects on the live backend: the one-request lifecycle and the changed-
//! scope, unknown-request, unprivileged-actor, and open-caller-action refusals.
//!
//! ## Owns
//! - The Postgres lifecycle and refusal assertions
//!
//! ## Does not own
//! - The request sequencing or state machine (`clio-compliance`)
//! - The shared private-schema fixture (`subject_request_pg_support`)
//! - The restart and bank-scope Postgres twins
//!   (`subject_request_tools_postgres_scenario_test`)
//!
//! ## Boundary
//!
//! Changes to the Postgres lifecycle or refusal evidence should normally
//! require changes here. Changes to the request state machine or the tool
//! decoding should not.
//!
//! Keep this module focused on the live-backend lifecycle and refusal evidence.

use clio_compliance::{PathOwner, PathSlot, PathState, RequestScope, RequestState};
use serde_json::json;

#[path = "subject_request_pg_support/mod.rs"]
mod subject_request_pg_support;

use subject_request_pg_support::{BANK, PgFixture, SUBJECT, call, load_record, store_item};

/// One request driven through start, status, copy, retry, events, and close on
/// the live backend, with the retry's effect counts and the final state.
#[test]
fn subject_request_tools_agree_on_one_request_on_postgres() {
    let Some(fixture) = PgFixture::open() else {
        return;
    };
    let state = &fixture.state;
    store_item(state, "itm-pg-lifecycle", SUBJECT, BANK);

    let request = "req-pg-lifecycle";
    let started = call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    assert_eq!(started["request_id"], json!(request));

    let status = call(
        state,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(status["request_id"], json!(request), "{status}");

    let copied = call(
        state,
        "subject_request_copy",
        json!({"bank": BANK, "request_id": request, "copy_id": "copy-pg-lifecycle",
               "custody": "clio", "owner": "harness", "required_action": "erase",
               "state": "done"}),
    );
    assert_eq!(copied["ok"], json!(true), "{copied}");
    assert_eq!(copied["request_id"], json!(request));

    let retried = call(
        state,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
    assert_eq!(retried["request_id"], json!(request));
    assert!(retried["action_id"].is_null(), "{retried}");
    assert_eq!(retried["effects_before"], json!(0), "{retried}");
    assert!(
        retried["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the retry commits the effect: {retried}"
    );
    assert!(
        state.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "the retried erasure destroys the subject key on Postgres"
    );

    let events = call(
        state,
        "subject_request_events",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(events["ok"], json!(true), "{events}");
    assert!(
        !events["rows"].as_array().expect("rows").is_empty(),
        "the retry records an event for the request: {events}"
    );

    let mut record = load_record(state, request);
    record.paths.insert(
        "erasure".to_owned(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    state
        .durable
        .save(&clio_compliance::subject_request_store_wiring::record_to_row(&record))
        .expect("save request");

    let closed = call(
        state,
        "subject_request_close",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(closed["ok"], json!(true), "{closed}");
    assert_eq!(closed["request_id"], json!(request));
    assert_eq!(closed["request_outcome"], json!("completed"));

    let final_status = call(
        state,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(final_status["request_id"], json!(request));
    assert_eq!(final_status["request_outcome"], json!("completed"));
}

/// A changed scope, an unknown request, an unprivileged actor, and an open
/// caller action each refuse on Postgres before an unsafe effect.
#[test]
fn subject_request_refusals_leave_the_request_unchanged_on_postgres() {
    let Some(fixture) = PgFixture::open() else {
        return;
    };
    let state = &fixture.state;
    let request = "req-pg-refusals";
    let first = call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": "pg-subject-a", "request_id": request}),
    );
    assert_eq!(first["ok"], json!(true), "{first}");

    let changed = call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": "pg-subject-b", "request_id": request}),
    );
    assert_eq!(changed["ok"], json!(false), "{changed}");
    assert!(
        matches!(load_record(state, request).scope,
                 RequestScope::Subject { ref subject_id, .. } if subject_id == "pg-subject-a"),
        "the stored scope is unchanged"
    );

    let unknown = call(
        state,
        "subject_request_status",
        json!({"bank": BANK, "request_id": "req-pg-does-not-exist"}),
    );
    assert_eq!(unknown["ok"], json!(false), "{unknown}");

    store_item(state, "itm-pg-unprivileged", "pg-subject-actor", BANK);
    let wrong_actor = call(
        state,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": "req-pg-actor", "subject_id": "pg-subject-actor",
               "legal_basis": "GDPR_ART_17", "confirm": true, "actor": "agent"}),
    );
    assert_eq!(wrong_actor["ok"], json!(false), "{wrong_actor}");
    assert_eq!(wrong_actor["code"], json!("forbidden"), "{wrong_actor}");
    assert!(
        !state
            .compliance
            .subject_dek_destroyed("pg-subject-actor")
            .unwrap(),
        "a refused retry destroys no key"
    );

    let copied = call(
        state,
        "subject_request_copy",
        json!({"bank": BANK, "request_id": request, "copy_id": "copy-pg-caller",
               "custody": "caller", "owner": "harness", "required_action": "erase",
               "state": "pending"}),
    );
    assert_eq!(copied["ok"], json!(true), "{copied}");
    let request_before = load_record(state, request);
    let closed = call(
        state,
        "subject_request_close",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(closed["ok"], json!(false), "{closed}");
    assert_eq!(closed["code"], json!("forbidden"), "{closed}");
    assert_eq!(
        load_record(state, request),
        request_before,
        "the open caller action leaves the stored request unchanged"
    );
    assert_eq!(load_record(state, request).state, RequestState::InProgress);
}
