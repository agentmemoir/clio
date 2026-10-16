// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` durable-request bindings end to end
//! at the CLI level over a private-schema Postgres store for the multi-bank
//! erasure scope: a partial scope refuses before any effect, while the exact
//! authorized scope with an offline peer runs but stays pending.
//!
//! ## Owns
//! - The Postgres CLI-level partial-scope and offline-peer scenario tests
//!
//! ## Does not own
//! - The command under test (`compliance_request_cli`)
//! - The SQLite twin (`compliance_request_cli_scope_tests`) and the Postgres
//!   lifecycle, refusal, and restart suites (sibling modules)
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
use clio_compliance::{PeerState, ProtectedAction};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_store::{PeerActionBinding, PeerActionKind, PgTestGuard, ScopedSchema, lock_pg_test};
use clio_types::Actor;
use serde_json::Value;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BANK: &str = "subject-request-pg-bank";
const PEER_BANK: &str = "subject-request-pg-peer-bank";
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

/// Open an erasure request for the shared subject through the CLI.
fn start(state: &Arc<McpState>, request: &str) -> (ExitCode, String, String) {
    run_json(
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

/// Add one frozen pending peer to the stored record, the fact the completion
/// gate reads that no tool sets without a multi-host deployment.
fn add_pending_peer(state: &McpState, request_id: &str, peer: &str) {
    let mut record = load_record(state, request_id);
    record.peers.insert(peer.to_owned(), PeerState::Pending);
    record.peer_banks.insert(peer.to_owned());
    state
        .durable
        .save(&record_to_row(&record))
        .expect("save request");
}

/// Begin one durable peer row under the derived action id, so a disposition
/// has a row to settle.
fn begin_peer_row(state: &McpState, request_id: &str, peer: &str) {
    let action_id = format!(
        "{}:{request_id}:{peer}",
        ProtectedAction::Erasure.producer_id()
    );
    state
        .peer_actions
        .begin(&PeerActionBinding {
            request_id,
            peer_id: peer,
            action_kind: PeerActionKind::Denial,
            action_id: &action_id,
        })
        .expect("begin peer row");
}

/// T101070-04a on Postgres: a scope that does not cover every bank sharing the
/// subject key refuses through the CLI before the intent or any effect.
#[test]
fn compliance_cli_pg_partial_bank_scope_refuses_before_effects() {
    let fixture = PgFixture::open();
    let s = &fixture.state;
    let request = "req-cli-pg-partial";
    store_item(s, "itm-cli-pg-partial-a", SUBJECT, BANK);
    store_item(s, "itm-cli-pg-partial-b", SUBJECT, PEER_BANK);
    let (code, out, err) = start(s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before = status_of(s, request);

    let (code, _out, err) = retry(s, request);
    assert_refusal(code, &err, "forbidden");
    assert!(
        !s.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "a refused partial-bank erasure destroys no key"
    );
    assert!(
        s.events.pending_actions().expect("pending").is_empty(),
        "a refused erasure writes no intent"
    );
    assert_eq!(
        status_of(s, request),
        before,
        "the stored request is unchanged by the refusal"
    );
}

/// T101070-04b on Postgres: the exact authorized scope with an offline peer
/// runs the effect but stays pending; status, retry, and peer-disposition use
/// the same request.
#[test]
fn compliance_cli_pg_authorized_scope_with_offline_peer_stays_pending() {
    let fixture = PgFixture::open();
    let s = &fixture.state;
    let request = "req-cli-pg-offline-peer";
    let peer = PEER_BANK;
    store_item(s, "itm-cli-pg-offline-a", SUBJECT, BANK);
    store_item(s, "itm-cli-pg-offline-b", SUBJECT, PEER_BANK);
    let (code, out, err) = start(s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    add_pending_peer(s, request, peer);
    begin_peer_row(s, request, peer);

    let (code, out, err) = retry(s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let ran = json_of(&out);
    assert!(
        ran["action_id"].is_null(),
        "nothing was pending before: {ran}"
    );
    assert!(
        ran["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the effect ran for the authorized scope: {ran}"
    );

    let (code, out, err) = run_json("peer-status", &["--request-id", request], s);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let peers = json_of(&out);
    assert_eq!(
        peers[peer],
        json!("pending"),
        "the offline peer stays pending: {peers}"
    );

    let (code, out, err) = run_json(
        "peer-retry",
        &["--request-id", request, "--legal-basis", "GDPR_ART_17"],
        s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["request_id"], json!(request));

    let (code, out, err) = run_json(
        "peer-disposition",
        &[
            "--request-id",
            request,
            "--peer-id",
            peer,
            "--disposition",
            "removal_evidence",
            "--proof-ref",
            "deletion-9",
        ],
        s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["request_id"], json!(request));

    let (code, out, err) = retry(s, request);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let retried = json_of(&out);
    assert!(
        retried["action_id"].is_string(),
        "the retry names the pending action: {retried}"
    );
    assert_eq!(
        retried["effects_before"], retried["effects_after"],
        "the retry repeats no protected effect: {retried}"
    );
    assert!(
        s.events.pending_actions().expect("pending").is_empty(),
        "the retry settles the action once the peer is evidenced"
    );
}
