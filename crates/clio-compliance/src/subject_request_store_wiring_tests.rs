// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the record/row mapping and restart-reload
//! coverage on SQLite: the round-trip through the row shape, restart replay
//! of the stored state, deadline and saved-path survival, scope-reuse
//! refusal without mutation, and closure refusals derived from a reloaded
//! record. The shared fixtures the sibling wiring test modules reopen live
//! here too.
//!
//! ## Owns
//! - The scratch SQLite directory fixture and the record builders
//! - The mapping and restart-reload assertions on SQLite
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The durable erasure seam (`subject_request_store_wiring_erase_tests.rs`)
//! - The live-Postgres twins (`subject_request_store_wiring_pg_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the record/row mapping or the restart reload stops
//! holding the stored facts verbatim.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::{EraseArgs, EraseVecSink};
use crate::subject_request::{
    PathOwner, PathSlot, PathState, RegisterOutcome, RequestCommand, RequestKind, RequestScope,
    RequestState, SubjectRequestLedger, SubjectRequestRecord,
};
use crate::subject_request_events::{ClosureBlock, accept_closure};
use crate::subject_request_store_wiring::{
    closure_inputs, record_to_row, reload_ledger, row_to_record,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, DekProvider, DurableRequestStore, ItemIdentity,
    LocalDevKms, SqliteStore, SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{
    AmError, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A scratch database directory removed when the guard drops (the
/// `subject_request_events_store_tests.rs` fixture shape).
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    pub(super) fn new() -> Self {
        Self(std::env::temp_dir().join(unique("clio-compliance-subject-request-wiring")))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A key provider for the file-backed scratch store.
pub(super) fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

/// Open the file-backed SQLite store at `path`; a reopen is the restart boundary.
pub(super) fn open_sqlite(path: &Path) -> SqliteStore {
    SqliteStore::open(path, kms()).expect("scratch sqlite opens")
}

/// A single-subject scope over a fresh bank and subject.
pub(super) fn subject_scope() -> RequestScope {
    RequestScope::Subject {
        bank_id: unique("wiring-bank"),
        subject_id: unique("subject"),
    }
}

/// The bank and subject a subject scope covers; the fixtures use no bank scope.
pub(super) fn scope_parts(scope: &RequestScope) -> (String, String) {
    match scope {
        RequestScope::Subject {
            bank_id,
            subject_id,
        } => (bank_id.clone(), subject_id.clone()),
        RequestScope::Bank { .. } => unreachable!("the fixture scope is a subject scope"),
    }
}

/// Open a request on a fresh ledger and start its work.
pub(super) fn started_record(request_id: &str, scope: &RequestScope) -> SubjectRequestRecord {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register(request_id, RequestKind::Erasure, scope, &[])
        .expect("the request opens");
    ledger
        .apply(request_id, &RequestCommand::Begin)
        .expect("the work starts");
    ledger
        .record(request_id)
        .expect("the record exists")
        .clone()
}

/// Persist `record`'s current state to the durable store.
pub(super) fn persist(durable: &dyn DurableRequestStore, record: &SubjectRequestRecord) {
    durable
        .save(&record_to_row(record).expect("the record encodes"))
        .expect("the row saves");
}

/// Compliance-store double: `erase_subject` fails while `fail` is set, marking
/// the DEK destroyed first; once `fail` clears the erasure completes. Counts
/// every erase call. The `banks` preset is the live bank set the effect would
/// fan out to; the seam reads it back from this same store. The failure mode
/// mirrors `MockEraseStore`.
pub(super) struct RetryStore {
    pub(super) fail: AtomicBool,
    dek_destroyed: AtomicBool,
    erase_calls: AtomicUsize,
    pub(super) banks: Vec<String>,
}

impl RetryStore {
    pub(super) fn failing() -> Self {
        Self {
            fail: AtomicBool::new(true),
            dek_destroyed: AtomicBool::new(false),
            erase_calls: AtomicUsize::new(0),
            banks: Vec::new(),
        }
    }

    pub(super) fn erase_calls(&self) -> usize {
        self.erase_calls.load(Ordering::SeqCst)
    }

    /// The live bank set, read from the effect's own store.
    pub(super) fn effect_bank_set(&self) -> Vec<String> {
        self.banks.clone()
    }
}

impl ComplianceStore for RetryStore {
    fn list_subject_items(&self, _: &str, _: &str) -> Result<Vec<ItemIdentity>, AmError> {
        Ok(vec![])
    }

    fn erase_subject(&self, _spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            self.dek_destroyed.store(true, Ordering::SeqCst);
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected partial failure",
            ));
        }
        Ok(SubjectEraseOutcome {
            item_ids: vec![],
            categories: vec![],
            item_hashes: vec![],
            subject_hash: "subject-hash".into(),
            vectors_purged: 0,
            assoc_edges_purged: 0,
            memtree_nodes_purged: 0,
            triples_purged: 0,
            derived_items_purged: 0,
            persona_context_rows_purged: 0,
            tombstones_written: 0,
        })
    }

    fn subject_dek_destroyed(&self, _subject: &str) -> Result<bool, AmError> {
        Ok(self.dek_destroyed.load(Ordering::SeqCst))
    }

    fn read_tombstones(&self, _: &str, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }

    fn list_tombstones(&self, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }
}

/// Erase arguments bound to `store`, `ctx`, and `sink`.
pub(super) fn erase_args<'a>(
    store: &'a dyn ComplianceStore,
    ctx: &'a OpContext,
    sink: &'a EraseVecSink,
    request_id: &'a str,
    subject_id: &'a str,
    salt: &'a [u8],
) -> EraseArgs<'a> {
    EraseArgs {
        store,
        maint: None,
        ctx,
        subject_id,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt,
        timestamp: "2026-01-01T00:00:00Z",
        sink,
    }
}

/// One semantic item carrying one data subject, for the fan-out fixture.
pub(super) fn wiring_item(bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: unique("wiring-item"),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("persona").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "secret"})),
        gist: Some("secret gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// A record encodes into the row shape and decodes back unchanged.
#[test]
fn a_record_round_trips_through_the_row_shape() {
    let scope = subject_scope();
    let mut record = started_record("req", &scope);
    record.deadline = Some("2026-03-01T00:00:00Z".into());
    record.paths.insert(
        "p1".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Saved {
                attempt_id: "att-1".into(),
                action_id: "act-1".into(),
            },
        },
    );
    let row = record_to_row(&record).expect("the record encodes");
    assert_eq!(row_to_record(&row).expect("the row decodes"), record);
}

/// A request started on SQLite replays its stored state after a restart.
#[test]
fn sqlite_a_started_request_replays_its_stored_state_after_a_restart() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    {
        let store = open_sqlite(&path);
        persist(&store, &started_record(&request_id, &scope));
    }
    let store = open_sqlite(&path);
    let mut ledger = reload_ledger(&store).expect("the ledger reloads");
    assert!(
        matches!(
            ledger.register(&request_id, RequestKind::Erasure, &scope, &[]),
            Ok(RegisterOutcome::Replayed {
                state: RequestState::InProgress
            })
        ),
        "the same id and scope must replay the stored state"
    );
}

/// A saved path slot restarts verbatim with its attempt and action ids.
#[test]
fn sqlite_a_saved_path_restarts_with_its_attempt_and_action_ids() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    {
        let store = open_sqlite(&path);
        let mut record = started_record(&request_id, &scope);
        record.paths.insert(
            "p1".into(),
            PathSlot {
                owner: PathOwner::Clio,
                state: PathState::Saved {
                    attempt_id: "att-1".into(),
                    action_id: "act-1".into(),
                },
            },
        );
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let restarted = ledger.record(&request_id).expect("the record reloads");
    assert_eq!(
        restarted.paths.get("p1").map(|slot| &slot.state),
        Some(&PathState::Saved {
            attempt_id: "att-1".into(),
            action_id: "act-1".into(),
        }),
        "the saved path must restart verbatim, with nothing auto-completed"
    );
}

/// An operator deadline supplied before the restart is reported unchanged.
#[test]
fn sqlite_a_supplied_deadline_survives_a_restart() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    {
        let store = open_sqlite(&path);
        let mut record = started_record(&request_id, &scope);
        record.deadline = Some("2026-03-01T00:00:00Z".into());
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let restarted = ledger.record(&request_id).expect("the record reloads");
    assert_eq!(restarted.deadline.as_deref(), Some("2026-03-01T00:00:00Z"));
}

/// A reused id bound to a changed scope refuses, leaving the row untouched.
#[test]
fn sqlite_a_reused_id_with_a_changed_scope_refuses_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    let record = started_record(&request_id, &scope);
    let stored_row = record_to_row(&record).expect("the record encodes");
    {
        let store = open_sqlite(&path);
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let mut ledger = reload_ledger(&store).expect("the ledger reloads");
    let changed = RequestScope::Bank {
        bank_id: unique("wiring-bank"),
    };
    let err = ledger
        .register(&request_id, RequestKind::Erasure, &changed, &[])
        .expect_err("a changed scope must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        store.load(&request_id).expect("load").expect("row exists"),
        stored_row,
        "the stored row must be untouched"
    );
}

/// A product path the reloaded record does not know blocks completion.
#[test]
fn sqlite_an_unknown_path_blocks_technical_completion_after_a_restart() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    {
        let store = open_sqlite(&path);
        persist(&store, &started_record(&request_id, &scope));
    }
    let store = open_sqlite(&path);
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let restarted = ledger.record(&request_id).expect("the record reloads");
    let inputs = closure_inputs(restarted, "unknown-path");
    assert!(
        matches!(
            accept_closure(&inputs),
            Err(ClosureBlock::UnknownProductPath)
        ),
        "an unknown path must block technical completion"
    );
}

/// A copy action still open on the reloaded record blocks closure.
#[test]
fn sqlite_an_open_copy_action_blocks_closure_after_a_restart() {
    use crate::subject_request::{RequestAction, RequestActionKind, RequestActionState};
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let scope = subject_scope();
    {
        let store = open_sqlite(&path);
        let mut record = started_record(&request_id, &scope);
        record.paths.insert(
            "p1".into(),
            PathSlot {
                owner: PathOwner::Clio,
                state: PathState::Pending,
            },
        );
        record.actions.insert(
            "copy-1".into(),
            RequestAction {
                kind: RequestActionKind::Copy,
                state: RequestActionState::Open,
            },
        );
        persist(&store, &record);
    }
    let store = open_sqlite(&path);
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let restarted = ledger.record(&request_id).expect("the record reloads");
    let inputs = closure_inputs(restarted, "p1");
    assert!(
        matches!(accept_closure(&inputs), Err(ClosureBlock::OpenCopyAction)),
        "an open copy action must block closure"
    );
}
