// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-surface scenario tests of the durable
//! subject-request tools that need a restartable store or a shared subject
//! bank: a failed event outcome retried after a reopen, and the multi-bank
//! erasure scope arms (a partial scope refused before any effect, and an
//! authorized scope with an offline peer that runs but stays pending).
//!
//! ## Owns
//! - The file-backed SQLite restart fixture and the outcome-failing event
//!   store double
//! - The restart, partial-scope, and offline-peer MCP tool scenario assertions
//!
//! ## Does not own
//! - The one-request lifecycle and refusal tests (`subject_request_tools_tests`)
//! - The request sequencing or state machine (`clio-compliance`)
//! - The live-Postgres twins (`tests/subject_request_tools_postgres_test.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The restart test owns its scratch database and key store. The confirmed
//! erasure still writes the deployment-resolved erasure journal, not a per-test
//! one: the erase boundary refuses a divergent document journal and this crate
//! forbids `unsafe_code`, so the override cannot be set in-process. The erasing
//! tests here share `erase_journal_serial` to serialize on that journal.

use super::subject_request_tools_tests::erase_journal_serial;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PeerState, ProtectedAction, RequestState};
use clio_store::{
    LocalEventIntent, LocalEventStore, LocalEventView, PeerActionBinding, PeerActionKind,
};
use clio_types::{Actor, AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BANK: &str = "subject-request-scenario-bank";
const SUBJECT: &str = "subject-request-scenario-subject";
const PEER_BANK: &str = "subject-request-scenario-peer-bank";

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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "am-mcp-subject-request-{tag}-{}-{nanos}-{sequence}",
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

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one item for `subject` in `bank`, so an erasure has work to do.
fn store_item(state: &McpState, id: &str, subject: &str, bank: &str) {
    let out = call(
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

/// A failed outcome write leaves one pending erasure action; a retry after the
/// store is reopened names it and reconciles it without repeating the effect.
#[test]
fn subject_request_retry_after_a_failed_outcome_reconciles_on_reopen() {
    let _serial = erase_journal_serial();
    let scratch = Scratch::new("restart");
    let db = scratch.path().join("store.db");
    let kms = scratch.path().join("clio.kms.json");
    let request = "req-scenario-restart";
    let pending;
    {
        let base = open_file_state(&db, &kms);
        let failing = Arc::new(FailingOutcomeEvents {
            inner: Arc::clone(&base.events),
        });
        let first = with_events(&base, failing);
        store_item(&first, "itm-scenario-restart", SUBJECT, BANK);
        let started = call(
            &first,
            "subject_request_start",
            json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
        );
        assert_eq!(started["ok"], json!(true), "{started}");
        let failed = call(
            &first,
            "subject_request_retry",
            json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
                   "legal_basis": "GDPR_ART_17", "confirm": true}),
        );
        assert_eq!(failed["ok"], json!(false), "{failed}");
        assert_eq!(failed["code"], json!("internal"), "{failed}");
        pending = first.events.pending_actions().expect("pending");
        assert_eq!(
            pending.len(),
            1,
            "one pending erasure action survives the failure: {pending:?}"
        );
    }

    // The command process restarts: reopen the same store and key store.
    let second = open_file_state(&db, &kms);
    let pending_id = pending[0].clone();
    let status = call(
        &second,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(status["request_id"], json!(request), "{status}");
    assert_eq!(
        status["technical_completion"],
        json!("pending"),
        "the restart reads the same request: {status}"
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
        &second,
        "subject_request_events",
        json!({"bank": BANK, "request_id": request}),
    );
    let rows = events["rows"].as_array().expect("rows");
    assert!(
        rows.iter()
            .any(|row| row["action_id"] == json!(pending_id) && row["state"] == json!("pending")),
        "the pending action id is named: {events}"
    );

    let retried = call(
        &second,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
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
        retried["effects_before"].as_u64().unwrap_or(0) >= 1,
        "the first attempt committed the effect: {retried}"
    );
    assert!(
        second.events.pending_actions().expect("pending").is_empty(),
        "the retry settles the pending action"
    );
}

/// A scope that does not cover every bank sharing the subject key refuses
/// before the intent or any effect.
#[test]
fn subject_request_partial_bank_scope_refuses_before_effects() {
    let s = state();
    let request = "req-scenario-partial";
    store_item(&s, "itm-scenario-partial-a", SUBJECT, BANK);
    store_item(&s, "itm-scenario-partial-b", SUBJECT, PEER_BANK);
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");

    let refused = call(
        &s,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("forbidden"), "{refused}");
    assert!(
        !s.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "a refused partial-bank erasure destroys no key"
    );
    assert!(
        s.events.pending_actions().expect("pending").is_empty(),
        "a refused erasure writes no intent"
    );
    assert_eq!(
        load_record(&s, request).state,
        RequestState::InProgress,
        "the stored request is unchanged by the refusal"
    );
}

/// The exact authorized bank set runs the effect, but an offline peer keeps
/// the request pending until the peer's evidence settles it.
#[test]
fn subject_request_authorized_scope_with_an_offline_peer_stays_pending() {
    let _serial = erase_journal_serial();
    let s = state();
    let request = "req-scenario-offline-peer";
    let peer = PEER_BANK;
    store_item(&s, "itm-scenario-offline-a", SUBJECT, BANK);
    store_item(&s, "itm-scenario-offline-b", SUBJECT, PEER_BANK);
    let started = call(
        &s,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "subject_id": SUBJECT, "request_id": request}),
    );
    assert_eq!(started["ok"], json!(true), "{started}");
    add_pending_peer(&s, request, peer);
    begin_peer_row(&s, request, peer);

    let ran = call(
        &s,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(ran["ok"], json!(true), "{ran}");
    assert!(
        ran["action_id"].is_null(),
        "nothing was pending before: {ran}"
    );
    assert!(
        ran["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the effect ran for the authorized scope: {ran}"
    );

    let peers = call(
        &s,
        "subject_request_peer_status",
        json!({"bank": BANK, "request_id": request}),
    );
    assert_eq!(
        peers[peer],
        json!("pending"),
        "the offline peer stays pending: {peers}"
    );

    let reconciled = call(
        &s,
        "subject_request_peer_retry",
        json!({"bank": BANK, "request_id": request, "legal_basis": "GDPR_ART_17"}),
    );
    assert_eq!(reconciled["ok"], json!(true), "{reconciled}");
    assert_eq!(reconciled["request_id"], json!(request));

    let disposed = call(
        &s,
        "subject_request_peer_disposition",
        json!({"bank": BANK, "request_id": request, "peer_id": peer,
               "disposition": "removal_evidence", "proof_ref": "deletion-9"}),
    );
    assert_eq!(disposed["ok"], json!(true), "{disposed}");
    assert_eq!(disposed["request_id"], json!(request));

    let retried = call(
        &s,
        "subject_request_retry",
        json!({"bank": BANK, "request_id": request, "subject_id": SUBJECT,
               "legal_basis": "GDPR_ART_17", "confirm": true}),
    );
    assert_eq!(retried["ok"], json!(true), "{retried}");
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
