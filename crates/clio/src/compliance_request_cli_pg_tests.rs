// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` durable-request bindings end to end
//! at the CLI level over a private-schema Postgres store: the one-request
//! lifecycle and the four refusal arms.
//!
//! ## Owns
//! - The Postgres CLI-level lifecycle and refusal scenario tests
//!
//! ## Does not own
//! - The command under test (`compliance_request_cli`)
//! - The SQLite twins (`compliance_request_cli_scenario_tests`) and the
//!   Postgres restart and scope suites (sibling modules)
//! - The request state machine (`clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only, and every test serializes on the shared Postgres test lock in a
//! private schema it owns. Never compiled into production builds.

use super::{CliIo, run_with_state};
use crate::cli_error::EXIT_OPERATIONAL;
use crate::cli_read_test_support::{json_of, tokens};
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PathOwner, PathSlot, PathState, RequestScope};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_store::{PgTestGuard, ScopedSchema, lock_pg_test};
use clio_types::Actor;
use serde_json::Value;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BANK: &str = "subject-request-pg-bank";
const SUBJECT: &str = "subject-request-pg-subject";

/// Scratch directories so far, so no two fixtures share a key-store path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory for one fixture's persistent key store.
fn scratch_dir(tag: &str) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "am-clio-subject-request-{tag}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// The shared `CLIO_DATABASE_URL` when it names Postgres, else the loopback
/// Postgres the test environment exposes under the shared lock.
fn database_url() -> String {
    match std::env::var("CLIO_DATABASE_URL") {
        Ok(url) if url.starts_with("postgres://") || url.starts_with("postgresql://") => url,
        _ => "postgres://clio:clio@127.0.0.1:34310/clio".to_owned(),
    }
}

/// Open a Postgres state over `url`, with `kms` as its persistent key store.
fn open_pg_state(url: &str, kms: &Path) -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "postgres".into(),
            database_url: url.to_owned(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            kms_path: Some(kms.to_path_buf()),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("open postgres state")
}

/// A private-schema Postgres state held with the schema and lock that own it.
struct PgFixture {
    state: Arc<McpState>,
    dir: PathBuf,
    _schema: ScopedSchema,
    _guard: PgTestGuard,
}

impl PgFixture {
    fn open() -> Self {
        let guard = lock_pg_test();
        let schema = ScopedSchema::new(&database_url());
        let dir = scratch_dir("pg-kms");
        let kms = dir.join("clio.kms.json");
        let state = open_pg_state(schema.url(), &kms);
        Self {
            state,
            dir,
            _schema: schema,
            _guard: guard,
        }
    }
}

impl Drop for PgFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

/// Postgres lifecycle: every request verb operates on one id and the
/// separated fields agree across start, status, copy, retry, events, close.
#[test]
fn compliance_cli_pg_lifecycle_verbs_agree_on_one_request() {
    let fixture = PgFixture::open();
    let s = &fixture.state;
    let request = "req-cli-pg-lifecycle";
    store_item(s, "itm-cli-pg-lifecycle", SUBJECT, BANK);

    let (code, out, err) = start(s, request, SUBJECT);
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

    let reported = status_of(s, request);
    assert_eq!(reported["state"], json!("InProgress"), "{reported}");

    let (code, out, err) = run_json(
        "request-copy",
        &[
            "--copy-id",
            "copy-cli-pg-lifecycle",
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
        s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["request_id"], json!(request));

    let (code, out, err) = retry(s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let retried = json_of(&out);
    assert_eq!(retried["request_id"], json!(request), "{retried}");
    assert_eq!(retried["action_id"], Value::Null, "{retried}");
    assert!(
        retried["effects_after"].as_u64().unwrap_or(0) >= 1,
        "{retried}"
    );

    let (code, out, err) = run_json("request-events", &["--request-id", request], s);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(
        !json_of(&out)["rows"].as_array().expect("rows").is_empty(),
        "the retry records an event"
    );

    // The closure gate needs a done product path; the request verbs do not set it.
    seed_done_path(s, request, "erasure");
    let (code, out, err) = run_json(
        "request-close",
        &["--request-id", request, "--product-path", "erasure"],
        s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let closed = json_of(&out);
    assert_eq!(closed["request_id"], json!(request), "{closed}");
    assert_eq!(closed["request_outcome"], json!("completed"), "{closed}");
}

/// Postgres refusals: a changed scope id, an unknown request, an
/// unprivileged actor, and an open caller action each refuse before an effect,
/// and the stored request stays unchanged.
#[test]
fn compliance_cli_pg_refusals_leave_the_request_unchanged() {
    let fixture = PgFixture::open();
    let s = &fixture.state;
    let request = "req-cli-pg-refusals";
    let (code, out, err) = start(s, request, "subject-pg-a");
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before = status_of(s, request);

    let (code, _out, err) = start(s, request, "subject-pg-b");
    assert_refusal(code, &err, "forbidden");
    assert_eq!(
        status_of(s, request),
        before,
        "the changed-scope refusal left the request unchanged"
    );
    assert!(
        matches!(load_record(s, request).scope, RequestScope::Subject { ref subject_id, .. } if subject_id == "subject-pg-a"),
    );

    let (code, _out, err) = run_json("request-status", &["--request-id", "req-cli-pg-missing"], s);
    assert_refusal(code, &err, "not_found");
    assert_eq!(status_of(s, request), before);

    let (code, _out, err) = run_json(
        "request-retry",
        &[
            "--request-id",
            request,
            "--subject-id",
            "subject-pg-a",
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
            "--actor",
            "agent",
        ],
        s,
    );
    assert_refusal(code, &err, "forbidden");
    assert!(
        !s.compliance.subject_dek_destroyed("subject-pg-a").unwrap(),
        "a refused retry destroys no key"
    );
    assert!(
        s.events.pending_actions().expect("pending").is_empty(),
        "a refused retry writes no intent"
    );
    assert_eq!(status_of(s, request), before);

    let (code, out, err) = run_json(
        "request-copy",
        &[
            "--copy-id",
            "copy-cli-pg-caller",
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
        s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before_close = status_of(s, request);
    let (code, _out, err) = run_json(
        "request-close",
        &["--request-id", request, "--product-path", "erasure"],
        s,
    );
    assert_refusal(code, &err, "forbidden");
    assert_eq!(
        status_of(s, request),
        before_close,
        "the refused close left the request unchanged"
    );
}
