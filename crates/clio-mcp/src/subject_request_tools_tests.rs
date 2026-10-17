// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-surface tests of the durable
//! subject-request tools: one request id bound across start, status, copy,
//! retry, events, and close; refusal of a changed scope, an unknown request,
//! and a wrong actor before effects; and the proof that the confirmed erasure
//! runs through the durable request store.
//!
//! ## Owns
//! - The SQLite MCP tests for the `subject_request_*` tools
//! - The `erase_request` reroute proof (a durable request row now backs the
//!   confirmed erasure) and its confirm gate
//!
//! ## Does not own
//! - The request sequencing or state machine (`clio-compliance`)
//! - The tool argument decoding (`subject_request_tools`)
//! - The same proof on Postgres (`tests/subject_request_tools_postgres_test.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The confirmed erasure writes the deployment-resolved erasure journal
//! (`CLIO_ERASURE_JOURNAL_FILE`, else the data directory), never a per-test
//! path: the erase boundary refuses a document journal that diverges from that
//! resolution, so an in-process test cannot redirect it, and this crate forbids
//! `unsafe_code`, so it cannot set the override either. `clio_store`'s
//! per-process lock registry refuses a second same-process holder at once, so
//! the tests that erase share `erase_journal_serial` to serialize, and no test
//! here asserts on the journal's content.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{
    PathOwner, PathSlot, PathState, RequestKind, RequestScope, VerificationResult,
};
use clio_types::Actor;
use serde_json::{Value, json};

const BANK: &str = "subject-request-test-bank";
const SUBJECT: &str = "subject-request-test-subject";

fn state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("open state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one item for `subject`, so an erasure has work to do.
fn store_item(state: &McpState, id: &str, subject: &str) {
    let out = call(
        state,
        "store",
        json!({
            "item": {"id": id, "subject_id": subject, "gist": "g",
                     "snapshot": {"entity": "g", "amount": 1, "date": "2024-01-01"}},
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user has g (1 note since 2024-01-01).",
            "bank": BANK
        }),
    );
    assert_eq!(out["ok"], json!(true), "seed item {id}: {out}");
}

/// Mark one product path done on the stored record, the one fact the closure
/// gate reads that the request verbs do not set themselves.
fn seed_done_path(state: &McpState, request_id: &str, path: &str) {
    let row = state
        .durable
        .load(request_id)
        .expect("load request")
        .expect("request row");
    let mut record = row_to_record(&row).expect("decode request");
    record.paths.insert(
        path.to_owned(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    state
        .durable
        .save(&record_to_row(&record))
        .expect("save request");
}

fn load_record(state: &McpState, request_id: &str) -> clio_compliance::SubjectRequestRecord {
    let row = state
        .durable
        .load(request_id)
        .expect("load request")
        .expect("request row");
    row_to_record(&row).expect("decode request")
}

/// Serializes the library tests that take the deployment-resolved erasure
/// journal's exclusive lock.
///
/// The erase boundary resolves that journal from the process environment and
/// accepts a document path only when it equals that resolution, so a test
/// cannot redirect it in-process. `clio_store` refuses a second same-process
/// holder at once, so an erasing test takes this mutex for its whole body.
static ERASE_JOURNAL_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Wait for the serialization; a poisoned lock from an earlier failing test is
/// recovered so it cannot mask this test's own result.
pub(super) fn erase_journal_serial() -> std::sync::MutexGuard<'static, ()> {
    ERASE_JOURNAL_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn subject_request_start_status_copy_retry_events_close_agree_on_one_request() {
    let _serial = erase_journal_serial();
    let s = state();
    store_item(&s, "itm-lifecycle", SUBJECT);
    let request = "req-lifecycle";

    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    assert_eq!(started["request_id"], json!(request));
    assert_eq!(started["technical_completion"], json!("pending"));
    assert_eq!(started["request_outcome"], json!("open"));

    let status = call(
        &s,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(status["request_id"], json!(request), "{status}");

    let copied = call(
        &s,
        "subject_request_copy",
        json!({"bank": BANK, "request_id": request, "copy_id": "copy-lifecycle",
               "custody": "clio", "owner": "harness", "required_action": "erase",
               "state": "done"}),
    );
    assert_eq!(copied["ok"], json!(true), "{copied}");
    assert_eq!(copied["request_id"], json!(request));

    let retried = call(
        &s,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
    assert_eq!(retried["request_id"], json!(request));
    assert!(
        retried["action_id"].is_null(),
        "nothing was pending before the first retry: {retried}"
    );
    assert_eq!(
        retried["effects_before"],
        json!(0),
        "the first retry starts with no committed tombstone: {retried}"
    );
    assert!(
        retried["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the first retry commits the effect: {retried}"
    );
    assert!(
        s.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "the retried erasure destroys the subject key"
    );

    let events = call(
        &s,
        "subject_request_events",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(events["ok"], json!(true), "{events}");
    let rows = events["rows"].as_array().expect("rows");
    assert!(
        !rows.is_empty(),
        "the retry records an event for the request"
    );
    for row in rows {
        assert!(row["action_id"].is_string(), "{row}");
        assert!(
            row.get("result").is_none(),
            "review rows are content-free: {row}"
        );
    }

    seed_done_path(&s, request, "erasure");
    let closed = call(
        &s,
        "subject_request_close",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(closed["ok"], json!(true), "{closed}");
    assert_eq!(closed["request_id"], json!(request));
    assert_eq!(closed["request_outcome"], json!("completed"));

    let final_status = call(
        &s,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(final_status["request_id"], json!(request));
    assert_eq!(final_status["request_outcome"], json!("completed"));
}

#[test]
fn subject_request_start_records_operator_verification_and_deadline() {
    let s = state();
    let request = "req-verification";
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request,
               "verification": "verified", "deadline": "2026-01-01T00:00:00Z"}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    assert_eq!(started["verification"], json!("verified"), "{started}");
    assert_eq!(
        started["deadline"],
        json!("2026-01-01T00:00:00Z"),
        "{started}"
    );
    let record = load_record(&s, request);
    assert_eq!(record.verification, Some(VerificationResult::Verified));
    assert_eq!(record.deadline.as_deref(), Some("2026-01-01T00:00:00Z"));
}

#[test]
fn subject_request_start_defaults_absent_verification_and_refuses_an_unknown_value() {
    let s = state();
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT,
               "request_id": "req-absent-verification"}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    assert!(started["verification"].is_null(), "{started}");
    assert!(started["deadline"].is_null(), "{started}");

    let refused = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT,
               "request_id": "req-bad-verification", "verification": "maybe"}),
    );
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("invalid_argument"), "{refused}");
    assert!(
        s.durable
            .load("req-bad-verification")
            .expect("load request")
            .is_none(),
        "a refused verification opens no request"
    );
}

#[test]
fn subject_request_changed_scope_and_unknown_request_refuse() {
    let s = state();
    let request = "req-changed-scope";
    let first = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": "subject-a", "request_id": request}),
    );
    assert_eq!(first["ok"], json!(true), "{first}");

    let changed = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": "subject-b", "request_id": request}),
    );
    assert_eq!(changed["ok"], json!(false), "{changed}");
    let record = load_record(&s, request);
    assert!(
        matches!(record.scope, RequestScope::Subject { ref subject_id, .. } if subject_id == "subject-a"),
        "the stored scope is unchanged: {:?}",
        record.scope
    );

    let unknown = call(
        &s,
        "subject_request_status",
        json!({"bank": BANK, "request_id": "req-does-not-exist"}),
    );
    assert_eq!(unknown["ok"], json!(false), "{unknown}");
}

#[test]
fn subject_request_open_caller_action_refuses_close_without_changes() {
    let s = state();
    let request = "req-open-caller";
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    let copied = call(
        &s,
        "subject_request_copy",
        json!({"bank": BANK, "request_id": request, "copy_id": "copy-caller",
               "custody": "caller", "owner": "harness", "required_action": "erase",
               "state": "pending"}),
    );
    assert_eq!(copied["ok"], json!(true), "{copied}");
    let request_before = load_record(&s, request);
    let copies_before = s.copies.load_all().expect("copies");

    let closed = call(
        &s,
        "subject_request_close",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(closed["ok"], json!(false), "{closed}");
    assert_eq!(closed["code"], json!("forbidden"), "{closed}");
    assert_eq!(
        load_record(&s, request),
        request_before,
        "the stored request is unchanged by the refusal"
    );
    assert_eq!(
        s.copies.load_all().expect("copies"),
        copies_before,
        "the caller copy row is unchanged by the refusal"
    );
}

#[test]
fn subject_request_wrong_actor_retry_refuses_before_effects() {
    let s = state();
    store_item(&s, "itm-wrong-actor", "subject-wrong-actor");
    let out = call(
        &s,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": "req-wrong-actor", "subject_id": "subject-wrong-actor",
               "legal_basis": "GDPR_ART_17", "confirm": true, "actor": "agent"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"), "{out}");
    assert!(
        !s.compliance
            .subject_dek_destroyed("subject-wrong-actor")
            .unwrap(),
        "a refused retry destroys no key"
    );
}

#[test]
fn subject_request_erase_request_uses_the_durable_route() {
    let _serial = erase_journal_serial();
    let s = state();
    store_item(&s, "itm-durable-route", "subject-durable-route");
    let request = "req-durable-route";

    let unconfirmed = call(
        &s,
        "erase_request",
        json!({"bank": BANK, "subject_id": "subject-durable-route", "legal_basis": "GDPR_ART_17",
               "request_id": request, "confirm": false}),
    );
    assert_eq!(unconfirmed["ok"], json!(false), "{unconfirmed}");
    assert!(
        s.durable.load(request).expect("load request").is_none(),
        "an unconfirmed erase opens no request"
    );

    let confirmed = call(
        &s,
        "erase_request",
        json!({"bank": BANK, "subject_id": "subject-durable-route", "legal_basis": "GDPR_ART_17",
               "request_id": request, "confirm": true}),
    );
    assert_eq!(confirmed["ok"], json!(true), "{confirmed}");

    // The old direct route wrote no durable request; the guarded route does.
    let record = load_record(&s, request);
    assert_eq!(record.kind, RequestKind::Erasure);
    assert!(
        s.compliance
            .subject_dek_destroyed("subject-durable-route")
            .unwrap(),
        "the confirmed erasure destroys the subject key"
    );
}

#[test]
fn subject_request_copy_and_exception_report_separate_fields() {
    let s = state();
    let request = "req-fields";
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "copy", "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");

    let copied = call(
        &s,
        "subject_request_copy",
        json!({"bank": BANK, "request_id": request, "copy_id": "copy-open",
               "custody": "operator", "owner": "harness", "required_action": "remove",
               "state": "pending"}),
    );
    assert_eq!(copied["ok"], json!(true), "{copied}");
    let pending = copied["pending_operator_actions"]
        .as_array()
        .expect("pending actions");
    assert!(
        pending.iter().any(|a| a["action_id"] == json!("copy-open")),
        "an open copy row is a pending operator action: {copied}"
    );

    let exception = call(
        &s,
        "subject_request_exception",
        json!({"bank": BANK, "request_id": request, "reason": "legal retention"}),
    );
    assert_eq!(exception["ok"], json!(true), "{exception}");
    assert_eq!(exception["exception"], json!("legal retention"));
    assert_eq!(
        exception["request_outcome"],
        json!("open"),
        "an open request stays open after an exception is recorded: {exception}"
    );
}
