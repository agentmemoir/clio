// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` durable-request bindings end to end
//! at the CLI level for a restart: a failed event outcome leaves one pending
//! action, a reopened file-backed store names it, and a CLI retry reconciles
//! it without repeating the protected effect.
//!
//! ## Owns
//! - The CLI-level restart/reconcile scenario test for `request-*`
//!
//! ## Does not own
//! - The command under test (`compliance_request_cli`)
//! - The lifecycle, refusal, and multi-bank scenarios (sibling test modules)
//! - The request state machine and its MCP twin (`clio-compliance`, `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The confirmed erasure writes the deployment-resolved erasure journal, not a
//! per-test one: the erase boundary refuses a divergent document journal and
//! this crate forbids `unsafe_code`, so the override cannot be set in-process.
//! The journal's `ExclusiveHold` serializes parallel writers.

use super::{CliIo, run_with_state};
use crate::cli_error::EXIT_OPERATIONAL;
use crate::cli_read_test_support::{json_of, tokens};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_store::{LocalEventIntent, LocalEventStore, LocalEventView};
use clio_types::{Actor, AmError, ErrorCode};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BANK: &str = "cli-subject-request-bank";
const SUBJECT: &str = "cli-subject-request-subject";

/// Scratch directories so far, so no two tests share a database path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One per-test scratch directory, removed when the guard drops.
///
/// `tempfile` is not a workspace dependency, so this is the repo's own fixture
/// pattern: a unique path under the OS temp directory with explicit cleanup.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "am-clio-subject-request-{tag}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An event store that delegates to the real one but fails the outcome write,
/// so a crash after the protected effect leaves the action pending.
struct FailingOutcomeEvents {
    inner: Arc<dyn LocalEventStore>,
}

impl LocalEventStore for FailingOutcomeEvents {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        self.inner.record_intent(intent)
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected outcome write failure",
        ))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.inner.pending_actions()
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.inner.list_events()
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.inner.prune_completed_before(cutoff)
    }
}

/// Open a file-backed SQLite state, so a second open reads the same rows.
fn open_file_state(db: &Path, kms: &Path) -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: db.to_string_lossy().into_owned(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            kms_path: Some(kms.to_path_buf()),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("open file state")
}

/// A clone of `base` whose event store is replaced, so one call fails its
/// outcome write while the durable rows stay in the shared store.
fn with_events(base: &McpState, events: Arc<dyn LocalEventStore>) -> Arc<McpState> {
    let mut clone = base.clone();
    clone.events = events;
    Arc::new(clone)
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

/// The requested request's status, read back through the CLI.
fn status_of(state: &Arc<McpState>, request: &str) -> serde_json::Value {
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

/// Store an item and run a start plus a confirmed retry whose outcome write
/// fails, returning the pending action ids the failure leaves behind.
fn fail_start_and_retry(state: &Arc<McpState>, request: &str, item: &str) -> Vec<String> {
    store_item(state, item, SUBJECT, BANK);
    let (code, out, err) = run_json(
        "request-start",
        &[
            "--request-id",
            request,
            "--kind",
            "erasure",
            "--subject-id",
            SUBJECT,
        ],
        state,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let (code, _out, err) = run_json(
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
    );
    assert_refusal(code, &err, "internal");
    state.events.pending_actions().expect("pending")
}

/// Restart reconcile: a failed outcome leaves one pending action; after the store is
/// reopened, the read surface names it and a retry reconciles it without
/// repeating the protected effect.
#[test]
fn compliance_cli_retry_reconciles_a_pending_action_across_a_restart() {
    let scratch = Scratch::new("restart");
    let db = scratch.path().join("store.db");
    let kms = scratch.path().join("clio.kms.json");
    let request = "req-cli-restart";
    let pending;
    {
        let base = open_file_state(&db, &kms);
        let failing = Arc::new(FailingOutcomeEvents {
            inner: Arc::clone(&base.events),
        });
        let first = with_events(&base, failing);
        pending = fail_start_and_retry(&first, request, "itm-cli-restart");
        assert_eq!(
            pending.len(),
            1,
            "one pending erasure action survives the failure: {pending:?}"
        );
    }

    // The command process restarts: reopen the same store and key store.
    let second = open_file_state(&db, &kms);
    let pending_id = pending[0].clone();

    let reported = status_of(&second, request);
    assert_eq!(reported["request_id"], json!(request), "{reported}");
    assert_eq!(
        reported["technical_completion"],
        json!("pending"),
        "the restart reads the same request: {reported}"
    );
    let pending_actions = reported["pending_operator_actions"]
        .as_array()
        .expect("pending operator actions");
    assert!(
        pending_actions
            .iter()
            .any(|action| action["action_id"] == json!(pending_id)),
        "the status names the pending action: {reported}"
    );

    // The event review still names the one pending action id after the restart.
    let (code, out, err) = run_json("request-events", &["--request-id", request], &second);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let events = json_of(&out);
    let rows = events["rows"].as_array().expect("rows");
    assert!(
        rows.iter()
            .any(|row| row["action_id"] == json!(pending_id) && row["state"] == json!("pending")),
        "the pending action id is named: {events}"
    );

    let (code, out, err) = run_json(
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
        &second,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let retried = json_of(&out);
    assert_eq!(
        retried["action_id"],
        json!(pending_id),
        "the retry names the pending action: {retried}"
    );
    assert_eq!(
        retried["effects_before"], retried["effects_after"],
        "the retry repeats no protected effect: {retried}"
    );
    assert!(
        second.events.pending_actions().expect("pending").is_empty(),
        "the retry settles the pending action"
    );
}

/// Explicit completion survives a process restart and close consumes that record.
#[test]
fn compliance_cli_completed_product_path_survives_restart() {
    let scratch = Scratch::new("path-complete");
    let db = scratch.path().join("store.db");
    let kms = scratch.path().join("keys.json");
    let args = ["--request-id", "request", "--product-path", "copy"];
    {
        let first = open_file_state(&db, &kms);
        let (code, out, err) = run_json(
            "request-start",
            &["--request-id", "request", "--kind", "copy"],
            &first,
        );
        assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
        let (code, out, err) = run_json("request-path-complete", &args, &first);
        assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
        assert_eq!(json_of(&out)["request_outcome"], json!("open"));
    }
    let second = open_file_state(&db, &kms);
    let (code, out, err) = run_json("request-close", &args, &second);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["request_outcome"], json!("completed"));
}
