// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared Postgres subject-request fixture
//! used by the split integration-test binaries: the private-schema state
//! builder, the outcome-failing event store double, and the small seeding and
//! record helpers the lifecycle, refusal, restart, and bank-scope suites share.
//!
//! ## Owns
//! - The fixture bank, subject, and peer-bank constants
//! - `PgFixture`, the private-schema state held with its schema and lock
//! - `FailingOutcomeEvents` and the item/record/peer seeding helpers
//!
//! ## Does not own
//! - The request sequencing or state machine (`clio-compliance`)
//! - The per-suite assertions (each integration-test binary owns those)
//!
//! ## Boundary
//!
//! Changes to the shared Postgres fixture should normally require changes here.
//! Changes to one binary's assertions should not require changes here.
//!
//! Keep this module focused on the shared Postgres subject-request fixture.
//
// The confirmed erasure writes the deployment-resolved erasure journal, not a
// per-test one: the erase boundary refuses a document journal that diverges
// from that resolution, and the workspace denies `unsafe_code`, so an
// integration test cannot set the override either. Each fixture owns its
// private schema and key-store directory; the journal's `ExclusiveHold`
// serializes parallel writers, and no test asserts on the journal's content.
//
// The same module is included by two integration-test binaries. A helper only
// one of them references must not fail the build, so cross-binary dead code is
// allowed here rather than deleting or duplicating a shared fixture.
#![allow(dead_code)]

use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PeerState, ProtectedAction};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_store::{
    LocalEventIntent, LocalEventStore, LocalEventView, PeerActionBinding, PeerActionKind,
};
use clio_types::{Actor, AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fixture bank for the Postgres subject-request suites.
pub const BANK: &str = "subject-request-pg-bank";
/// Fixture subject for the Postgres subject-request suites.
pub const SUBJECT: &str = "subject-request-pg-subject";
/// The second bank sharing the fixture subject's items.
pub const PEER_BANK: &str = "subject-request-pg-peer-bank";

/// Scratch directories so far, so no two fixtures share a key-store path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory for one fixture's persistent key store.
fn scratch_dir(tag: &str) -> PathBuf {
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
    dir
}

/// The shared `CLIO_DATABASE_URL`, but only when it points at a Postgres
/// backend. A missing or non-Postgres value fails closed unless the operator
/// opted out, which prints a note and makes the caller return early.
pub fn postgres_url() -> Option<String> {
    clio_store::pg_skip::pg_url_or_skip("the subject-request Postgres fixtures")
}

/// Open a Postgres state over `url`, with `kms` as its persistent key store.
pub fn open_pg_state(url: &str, kms: &Path) -> Arc<McpState> {
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
pub struct PgFixture {
    /// The state the suite drives.
    pub state: Arc<McpState>,
    /// The private schema's connection string.
    pub url: String,
    kms: PathBuf,
    dir: PathBuf,
    _schema: clio_store::ScopedSchema,
    _guard: clio_store::PgTestGuard,
}

impl PgFixture {
    /// Open the live private-schema fixture under the shared Postgres lock, or
    /// `None` when no Postgres `CLIO_DATABASE_URL` is configured so the caller
    /// returns early instead of failing.
    pub fn open() -> Option<Self> {
        let url = postgres_url()?;
        let guard = clio_store::lock_pg_test();
        let schema = clio_store::ScopedSchema::new(&url);
        let url = schema.url().to_owned();
        let dir = scratch_dir("pg-kms");
        let kms = dir.join("clio.kms.json");
        let state = open_pg_state(&url, &kms);
        Some(Self {
            state,
            url,
            kms,
            dir,
            _schema: schema,
            _guard: guard,
        })
    }

    /// Open a second state over the same private schema and key store, a
    /// process restart.
    pub fn reopen(&self) -> Arc<McpState> {
        open_pg_state(&self.url, &self.kms)
    }
}

impl Drop for PgFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An event store that delegates to the real one but fails the outcome write.
pub struct FailingOutcomeEvents {
    inner: Arc<dyn LocalEventStore>,
}

impl FailingOutcomeEvents {
    /// Wrap the real event store of `state`.
    pub fn over(state: &McpState) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::clone(&state.events),
        })
    }
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

/// Route one subject-request tool call through the in-process dispatcher.
#[allow(clippy::needless_pass_by_value)]
pub fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one item for `subject` in `bank`, so an erasure has work to do.
pub fn store_item(state: &McpState, id: &str, subject: &str, bank: &str) {
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

/// Decode the stored request record.
pub fn load_record(state: &McpState, request_id: &str) -> clio_compliance::SubjectRequestRecord {
    let row = state
        .durable
        .load(request_id)
        .expect("load request")
        .expect("request row");
    row_to_record(&row).expect("decode request")
}

/// Add one frozen pending peer to the stored record, the fact the completion
/// gate reads that no tool sets without a multi-host deployment.
pub fn add_pending_peer(state: &McpState, request_id: &str, peer: &str) {
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
pub fn begin_peer_row(state: &McpState, request_id: &str, peer: &str) {
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
