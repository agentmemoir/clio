// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` durable-request bindings end to end
//! at the CLI level for the one-request lifecycle: every request verb operates
//! on the same id and the separated status fields agree, and the four refusal
//! arms leave the stored request unchanged.
//!
//! ## Owns
//! - The CLI-level lifecycle and refusal scenario tests for `request-*`
//!
//! ## Does not own
//! - The command under test (`compliance_request_cli`)
//! - The restart and multi-bank scenarios
//!   (`compliance_request_cli_restart_tests`,
//!   `compliance_request_cli_scope_tests`)
//! - The request state machine and its MCP twin (`clio-compliance`, `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{CliIo, run_with_state};
use crate::cli_error::EXIT_OPERATIONAL;
use crate::cli_read_test_support::{json_of, tokens};
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PathOwner, PathSlot, PathState, RequestScope};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_types::Actor;
use serde_json::Value;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

const BANK: &str = "cli-subject-request-bank";
const SUBJECT: &str = "cli-subject-request-subject";

/// Open in-memory SQLite state with harness authority, as the MCP twin does.
fn state() -> Arc<McpState> {
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

/// Run one request verb through the CLI dispatch against injected state.
fn run(sub: &str, args: &[&str], state: &Arc<McpState>) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        run_with_state(sub, tokens(args), state, &mut io)
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// Run one verb requesting JSON output.
fn run_json(sub: &str, args: &[&str], state: &Arc<McpState>) -> (ExitCode, String, String) {
    let mut all = args.to_vec();
    all.extend(["-o", "json"]);
    run(sub, &all, state)
}

/// Open an erasure request for `subject` through the CLI.
fn start(state: &Arc<McpState>, request: &str, subject: &str) -> (ExitCode, String, String) {
    run_json(
        "request-start",
        &[
            "--request-id",
            request,
            "--kind",
            "erasure",
            "--subject-id",
            subject,
        ],
        state,
    )
}

/// Retry a confirmed erasure through the CLI.
fn retry(state: &Arc<McpState>, request: &str) -> (ExitCode, String, String) {
    run_json(
        "request-retry",
        &[
            "--request-id",
            request,
            "--subject-id",
            SUBJECT,
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
        ],
        state,
    )
}

/// The requested request's status, read back through the CLI.
fn status_of(state: &Arc<McpState>, request: &str) -> Value {
    let (code, out, err) = run_json("request-status", &["--request-id", request], state);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    json_of(&out)
}

/// Assert a refused verb: exit 1 and the refused envelope's own code.
fn assert_refusal(code: ExitCode, err: &str, expected_code: &str) {
    assert_eq!(code, ExitCode::from(EXIT_OPERATIONAL), "{err}");
    let envelope = json_of(err);
    assert_eq!(envelope["ok"], json!(false), "{envelope}");
    assert_eq!(envelope["code"], json!(expected_code), "{envelope}");
}

/// Store one item for `subject` in `bank`, so an erasure has work to do.
fn store_item(state: &McpState, id: &str, subject: &str, bank: &str) {
    let out = crate::cli_read_test_support::call(
        state,
        "store",
        json!({
            "item": {"id": id, "subject_id": subject, "gist": "g",
                     "snapshot": {"entity": "g", "amount": 1, "date": "2024-01-01"}},
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user has g (1 note since 2024-01-01).",
            "bank": bank
        }),
    );
    assert_eq!(out["ok"], json!(true), "seed item {id}: {out}");
}

fn load_record(state: &McpState, request_id: &str) -> clio_compliance::SubjectRequestRecord {
    let row = state
        .durable
        .load(request_id)
        .expect("load request")
        .expect("request row");
    row_to_record(&row).expect("decode request")
}

/// Mark one product path done on the stored record, the one fact the closure
/// gate reads that the request verbs do not set themselves.
fn seed_done_path(state: &McpState, request_id: &str, path: &str) {
    let mut record = load_record(state, request_id);
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

/// One request id: every request verb operates on one id and the separated fields
/// agree across start, status, copy, retry, events, and close.
#[test]
fn compliance_cli_lifecycle_verbs_agree_on_one_request() {
    let s = state();
    let request = "req-cli-lifecycle";
    store_item(&s, "itm-cli-lifecycle", SUBJECT, BANK);

    let (code, out, err) = start(&s, request, SUBJECT);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let started = json_of(&out);
    assert_eq!(started["request_id"], json!(request), "{started}");
    assert_eq!(
        started["technical_completion"],
        json!("pending"),
        "{started}"
    );
    assert_eq!(started["request_outcome"], json!("open"), "{started}");
    assert!(started["pending_operator_actions"].is_array(), "{started}");

    let reported = status_of(&s, request);
    assert_eq!(reported["state"], json!("InProgress"), "{reported}");
    assert_eq!(
        reported["technical_completion"],
        json!("pending"),
        "{reported}"
    );

    let (code, out, err) = run_json(
        "request-copy",
        &[
            "--copy-id",
            "copy-cli-lifecycle",
            "--request-id",
            request,
            "--custody",
            "clio",
            "--owner",
            "harness",
            "--required-action",
            "erase",
            "--state",
            "done",
        ],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["request_id"], json!(request));

    let (code, out, err) = retry(&s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let retried = json_of(&out);
    assert_eq!(retried["request_id"], json!(request), "{retried}");
    assert_eq!(retried["action_id"], Value::Null, "{retried}");
    assert!(
        retried["effects_after"].as_u64().unwrap_or(0) >= 1,
        "{retried}"
    );

    let (code, out, err) = run_json("request-events", &["--request-id", request], &s);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let events = json_of(&out);
    assert!(
        !events["rows"].as_array().expect("rows").is_empty(),
        "the retry records an event: {events}"
    );

    // The closure gate needs a done product path; the request verbs do not set it.
    seed_done_path(&s, request, "erasure");
    let (code, out, err) = run_json(
        "request-close",
        &["--request-id", request, "--product-path", "erasure"],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let closed = json_of(&out);
    assert_eq!(closed["request_id"], json!(request), "{closed}");
    assert_eq!(closed["request_outcome"], json!("completed"), "{closed}");

    assert_eq!(
        status_of(&s, request)["request_outcome"],
        json!("completed")
    );
}

/// Safe refusals: a changed scope id, an unknown request, an unprivileged actor,
/// and an open caller action each refuse before an effect, and the stored
/// request stays unchanged.
#[test]
fn compliance_cli_refusals_leave_the_request_unchanged() {
    let s = state();
    let request = "req-cli-refusals";
    let (code, out, err) = start(&s, request, "subject-cli-a");
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before = status_of(&s, request);

    // 1. The same id bound to a changed scope.
    let (code, _out, err) = start(&s, request, "subject-cli-b");
    assert_refusal(code, &err, "forbidden");
    assert_eq!(
        status_of(&s, request),
        before,
        "the changed-scope refusal left the request unchanged"
    );
    assert!(
        matches!(load_record(&s, request).scope, RequestScope::Subject { ref subject_id, .. } if subject_id == "subject-cli-a"),
        "the stored scope still names the first subject"
    );

    // 2. An unknown request id.
    let (code, _out, err) = run_json("request-status", &["--request-id", "req-cli-missing"], &s);
    assert_refusal(code, &err, "not_found");
    assert_eq!(status_of(&s, request), before);

    // 3. An unprivileged actor refuses the retry before any effect.
    let (code, _out, err) = run_json(
        "request-retry",
        &[
            "--request-id",
            request,
            "--subject-id",
            "subject-cli-a",
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
            "--actor",
            "agent",
        ],
        &s,
    );
    assert_refusal(code, &err, "forbidden");
    assert!(
        !s.compliance.subject_dek_destroyed("subject-cli-a").unwrap(),
        "a refused retry destroys no key"
    );
    assert!(
        s.events.pending_actions().expect("pending").is_empty(),
        "a refused retry writes no intent"
    );
    assert_eq!(status_of(&s, request), before);

    // 4. An open caller action refuses the close.
    let (code, out, err) = run_json(
        "request-copy",
        &[
            "--copy-id",
            "copy-cli-caller",
            "--request-id",
            request,
            "--custody",
            "caller",
            "--owner",
            "harness",
            "--required-action",
            "erase",
            "--state",
            "pending",
        ],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before_close = status_of(&s, request);
    let (code, _out, err) = run_json(
        "request-close",
        &["--request-id", request, "--product-path", "erasure"],
        &s,
    );
    assert_refusal(code, &err, "forbidden");
    assert_eq!(
        status_of(&s, request),
        before_close,
        "the refused close left the request unchanged"
    );
}
