// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving every real review-file export binding
//! on both backends: that the review export writes its custody row and its
//! redacted ordered output, that a failed intent refuses before the effect and a
//! failed outcome reconciles to one terminal result without redelivery.
//!
//! ## Owns
//! - The review-export binding matrix on SQLite and Postgres
//! - The custody-row, redacted-output, pre-effect refusal, and one-outcome
//!   assertions
//!
//! ## Does not own
//! - The export custody primitive and reconcile (see `clio-compliance`)
//! - The CLI driver and the Postgres gate (see `cli` and `support_pg`)
//!
//! ## Boundary
//!
//! Changes to which review-export bindings this claim proves should normally
//! require changes here. Changes to the custody primitive should not.
//!
//! Keep this module focused on the review-export claim.
use crate::cli::{CliOutput, clio_command, run_cli};
use crate::fixtures::{
    BANK, DENIED_MARKER, NOW, backup_dir, journal_file, open_pg, open_sqlite, scratch, unique,
};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::{
    DestinationCustodyStore, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    PgTestGuard, ScopedSchema,
};
use clio_compliance::{BindingControls, DeliverySpec, LocalReviewQuery, write_review_export};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
/// The bounded wait for one MCP response or a clean server exit.
const MCP_WAIT: Duration = Duration::from_secs(30);
/// The store surface one review-export scenario drives.
trait Store: LocalEventStore + DestinationCustodyStore {}
impl<T: LocalEventStore + DestinationCustodyStore> Store for T {}
/// One isolated deployment: a fresh SQLite file or a private Postgres schema.
struct Target {
    postgres: bool,
    db: String,
    kms: PathBuf,
    journal: PathBuf,
    backups: PathBuf,
    scratch: crate::common::Scratch,
}
impl Target {
    fn sqlite(tag: &str) -> Self {
        Self::build(false, tag, String::new())
    }
    fn postgres(tag: &str, url: &str) -> Self {
        Self::build(true, tag, url.to_owned())
    }
    fn build(postgres: bool, tag: &str, db: String) -> Self {
        let scratch = scratch(tag);
        let kms = scratch.join("key");
        let journal = journal_file(&scratch, "journal/erasure.json");
        let backups = backup_dir(&scratch);
        let db = if postgres {
            db
        } else {
            scratch.join("store.db").display().to_string()
        };
        Self {
            postgres,
            db,
            kms,
            journal,
            backups,
            scratch,
        }
    }
    fn open(&self) -> Box<dyn Store> {
        if self.postgres {
            Box::new(open_pg(&self.db, &self.kms))
        } else {
            Box::new(open_sqlite(Path::new(&self.db), &self.kms))
        }
    }
    fn args(&self) -> Vec<&str> {
        let backend = if self.postgres { "postgres" } else { "sqlite" };
        vec!["--db", &self.db, "--backend", backend]
    }
}
fn postgres_target(tag: &str) -> Option<(Target, ScopedSchema, PgTestGuard)> {
    let url = pg_url();
    if !require_postgres(postgres_reachable(&url)) {
        eprintln!("scenario_review_export: skip {tag}: no Postgres backend");
        return None;
    }
    let lock = lock_pg();
    let schema = ScopedSchema::new(&url);
    let target = Target::postgres(tag, schema.url());
    Some((target, schema, lock))
}
fn intent(action_id: &str) -> LocalEventIntent<'_> {
    LocalEventIntent {
        action_id,
        actor: "harness",
        scope: "review_scenario",
        kind: "local_probe",
        event_time: NOW,
    }
}
fn spec<'a>(action_id: &'a str, destination: &'a Path, staging: &'a Path) -> DeliverySpec<'a> {
    DeliverySpec {
        action_id,
        destination,
        staging_dir: staging,
        custody_owner: "clio",
        required_action: "delivery",
        now: NOW,
    }
}
/// Deliver one review export through the seam both real bindings delegate to.
fn write(
    events: &dyn LocalEventStore,
    custody: &dyn DestinationCustodyStore,
    action_id: &str,
    destination: &Path,
) -> Result<(), AmError> {
    let ctx = OpContext::new(Bank::new(BANK.to_owned()), Actor::Harness);
    let query = LocalReviewQuery {
        limit: 1000,
        cursor: None,
        request_id: None,
    };
    let controls = BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    };
    // Unique per destination and under the test's `Scratch`, which removes it.
    let staging = destination.with_extension("staging");
    write_review_export(
        &ctx,
        events,
        custody,
        &query,
        &controls,
        &spec(action_id, destination, &staging),
    )
}
/// Seed one allowed (with a marker), one pending, and one denied row.
fn seed_events(store: &dyn LocalEventStore, ids: &[String; 3]) {
    store.record_intent(&intent(&ids[0])).expect("first intent");
    store
        .record_outcome(&ids[0], false, DENIED_MARKER)
        .expect("marker outcome");
    store
        .record_intent(&intent(&ids[1]))
        .expect("pending intent");
    store.record_intent(&intent(&ids[2])).expect("third intent");
    store
        .record_outcome(&ids[2], true, "denied reason")
        .expect("third outcome");
}
/// The event rows for one action, read back from a (possibly reopened) store.
fn read_events(store: &dyn LocalEventStore, action_id: &str) -> Vec<LocalEventView> {
    store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|e| e.action_id == action_id)
        .collect()
}
fn pending(store: &dyn LocalEventStore) -> Vec<String> {
    store.pending_actions().expect("pending")
}
/// An event writer that fails only the intent write or only the outcome write.
struct EventWriteFault<'a> {
    inner: &'a dyn LocalEventStore,
    fail_intent: bool,
    fail_outcome: bool,
}
impl<'a> EventWriteFault<'a> {
    fn new(inner: &'a dyn LocalEventStore, fail_intent: bool, fail_outcome: bool) -> Self {
        Self {
            inner,
            fail_intent,
            fail_outcome,
        }
    }
}
impl LocalEventStore for EventWriteFault<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        if self.fail_intent {
            return Err(AmError::new(ErrorCode::Internal, "injected intent failure"));
        }
        self.inner.record_intent(intent)
    }
    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        if self.fail_outcome {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected outcome failure",
            ));
        }
        self.inner.record_outcome(action_id, denied, result)
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
/// Run the real CLI review export to `destination`.
fn run_cli_export(target: &Target, destination: &Path) -> CliOutput {
    let out = destination.display().to_string();
    let mut args = vec!["compliance", "review-export", "--out", out.as_str()];
    args.extend(["--actor", "harness", "--limit", "1000", "--output", "json"]);
    args.extend(target.args());
    run_cli(&target.journal, &target.kms, &target.backups, &args)
}
/// Write one JSON-RPC frame and flush it.
fn mcp_write(stdin: &mut ChildStdin, frame: &Value) {
    writeln!(stdin, "{frame}").expect("write frame");
    stdin.flush().expect("flush frame");
}
/// Write one request frame and return its response, skipping other frames.
fn mcp_exchange(
    stdin: &mut ChildStdin,
    responses: &mpsc::Receiver<String>,
    id: i64,
    frame: &Value,
) -> Value {
    mcp_write(stdin, frame);
    loop {
        let line = responses
            .recv_timeout(MCP_WAIT)
            .unwrap_or_else(|error| panic!("no MCP response for id {id} in {MCP_WAIT:?}: {error}"));
        let parsed: Value = serde_json::from_str(&line).expect("JSON-RPC response");
        if parsed.get("id").and_then(Value::as_i64) == Some(id) {
            return parsed;
        }
    }
}
struct Reaper(Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
/// Run the real MCP stdio `review_export` tool to `destination`.
fn run_mcp_export(target: &Target, destination: &Path) {
    let mut command = clio_command(&target.journal, &target.kms, &target.backups);
    command
        .args(["mcp", "stdio"])
        .args(target.args())
        .stdin(Stdio::piped());
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = Reaper(command.spawn().expect("spawn clio mcp stdio"));
    let mut stdin = child.0.stdin.take().expect("mcp stdin");
    let stdout = child.0.stdout.take().expect("mcp stdout");
    let (sender, responses) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                   "clientInfo": {"name": "cross-path", "version": "0"}}
    });
    mcp_exchange(&mut stdin, &responses, 1, &init);
    mcp_write(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "review_export",
                   "arguments": {"destination": destination.display().to_string(),
                                 "limit": 1000, "actor": "harness"}}
    });
    let result = mcp_exchange(&mut stdin, &responses, 2, &call)["result"].clone();
    assert_eq!(result["isError"], json!(false), "{result}");
    assert_eq!(result["structuredContent"]["ok"], json!(true), "{result}");
    drop(stdin);
    let status = child.0.wait().expect("wait for clio mcp stdio");
    assert!(status.success(), "clio mcp stdio exited {status:?}");
}
/// Assert the destination is the redacted, ordered output under one custody row.
fn assert_export(target: &Target, ids: &[String; 3], destination: &Path) {
    let text = std::fs::read_to_string(destination).expect("read the review export");
    assert!(!text.contains(DENIED_MARKER), "the redacted copy: {text}");
    let mut lines = text.lines();
    for (i, state) in ["allowed", "pending", "denied"].iter().enumerate() {
        let want = format!("\t{}\t{state}", ids[i]);
        assert!(
            lines.any(|line| line.ends_with(&want)),
            "row {i} out of order: {text}"
        );
    }
    let store = target.open();
    let rows: Vec<_> = store
        .load_all()
        .expect("custody rows")
        .into_iter()
        .filter(|row| row.destination_path == destination.display().to_string())
        .collect();
    assert_eq!(rows.len(), 1, "one custody row per destination");
    let row = &rows[0];
    assert_eq!(
        (row.state.as_str(), row.custody_owner.as_str()),
        ("delivered", "clio")
    );
    assert!(
        row.action_id.starts_with("review-export-"),
        "action: {}",
        row.action_id
    );
    let events = read_events(store.as_ref(), &row.action_id);
    assert_eq!(events.len(), 1, "one event row per export action");
    assert_eq!(events[0].state, LocalEventState::Allowed);
    assert!(
        !store
            .pending_actions()
            .expect("pending")
            .contains(&row.action_id)
    );
}
/// Both real bindings deliver the same redacted, ordered, custody-recorded file.
fn public_route_matrix(target: &Target) {
    let ids = [unique("rev-a"), unique("rev-b"), unique("rev-c")];
    {
        let store = target.open();
        seed_events(store.as_ref(), &ids);
    }
    let cli_destination = target.scratch.join("cli-review.txt");
    let output = run_cli_export(target, &cli_destination);
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_export(target, &ids, &cli_destination);
    let mcp_destination = target.scratch.join("mcp-review.txt");
    run_mcp_export(target, &mcp_destination);
    assert_export(target, &ids, &mcp_destination);
}
/// A failed intent refuses before any effect.
fn intent_failure(target: &Target) {
    let store = target.open();
    let before = store.list_events().expect("events").len();
    let destination = target.scratch.join("intent-refused.txt");
    let action_id = unique("rev-intent");
    let fault = EventWriteFault::new(store.as_ref(), true, false);
    let error = write(&fault, store.as_ref(), &action_id, &destination)
        .expect_err("an intent failure refuses");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!destination.exists(), "no destination write");
    assert!(store.load(&action_id).expect("load").is_none(), "no row");
    assert_eq!(store.list_events().expect("events").len(), before);
    assert!(
        !store
            .pending_actions()
            .expect("pending")
            .contains(&action_id)
    );
}
/// A failed outcome keeps one copy and one pending action; a real restart
/// reconciles one terminal outcome and a repeat call redelivers nothing.
fn outcome_failure(target: &Target) {
    let seed = unique("rev-seed");
    let destination = target.scratch.join("outcome-copy.txt");
    let action_id = unique("rev-outcome");
    let before = {
        let store = target.open();
        store.record_intent(&intent(&seed)).expect("seed intent");
        store
            .record_outcome(&seed, false, "seed allowed")
            .expect("seed outcome");
        let fault = EventWriteFault::new(store.as_ref(), false, true);
        let error = write(&fault, store.as_ref(), &action_id, &destination)
            .expect_err("an outcome failure is reported");
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            pending(store.as_ref()).contains(&action_id),
            "the failed action waits"
        );
        let events = read_events(store.as_ref(), &action_id);
        assert_eq!(events.len(), 1, "one intent row for the action");
        assert_eq!(events[0].state, LocalEventState::Pending);
        let row = store.load(&action_id).expect("load").expect("custody row");
        assert_eq!(row.state, "delivered", "the one copy is recorded");
        let copy = std::fs::read(&destination).expect("read the delivered copy");
        assert!(
            String::from_utf8_lossy(&copy).contains(&seed),
            "effect before failure"
        );
        copy
    };
    let restarted = target.scratch.join("after-restart.txt");
    let output = run_cli_export(target, &restarted);
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let store = target.open();
    assert!(
        !pending(store.as_ref()).contains(&action_id),
        "restart reconciles"
    );
    let events = read_events(store.as_ref(), &action_id);
    assert_eq!(events.len(), 1, "one reconciled outcome, not two rows");
    assert_eq!(events[0].state, LocalEventState::Allowed);
    assert_eq!(
        std::fs::read(&destination).expect("copy"),
        before,
        "no redelivery"
    );
    let repeat = run_cli_export(target, &destination);
    assert_ne!(repeat.code, Some(0), "an existing destination is refused");
    assert_eq!(
        std::fs::read(&destination).expect("copy"),
        before,
        "no overwrite"
    );
    assert_eq!(read_events(store.as_ref(), &action_id).len(), 1);
    assert!(!pending(store.as_ref()).contains(&action_id));
}
/// Run the full review-export matrix for one deployment.
fn matrix(target: &Target) {
    public_route_matrix(target);
    intent_failure(target);
    outcome_failure(target);
}
#[test]
fn sqlite_review_export_matrix_covers_public_intent_and_outcome() {
    matrix(&Target::sqlite("sqlite-matrix"));
}
#[test]
fn postgres_review_export_matrix_covers_public_intent_and_outcome() {
    let Some((target, _schema, _lock)) = postgres_target("pg-matrix") else {
        return;
    };
    matrix(&target);
}
