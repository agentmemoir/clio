// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restart-recovery coverage: the
//! reconciliation of pending erasure actions and its refusal-class split, the
//! refresh-only resume a restart still owes, the hard-effect and key-probe
//! fault classes that must stay pending, and the restart boundary itself.
//!
//! ## Owns
//! - The reconcile, refresh-only, and faulted-flow assertions and the faulted
//!   compliance-store double
//!
//! ## Does not own
//! - The reconcile (`erasure_reconcile.rs`), the flow (`erasure_retry.rs`), and
//!   the shared fixtures (`erasure_retry_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a restart stops settling by effect evidence, starts
//! denying a hard error, or stops refreshing the derived state.
//!
//! Test-only. Never compiled into production builds.

use super::erasure_retry_tests::{
    Attempt, action_id, action_rows, live_set, load_row, persisted, tombstone_count,
};
use super::subject_request_store_wiring_tests::{
    RetryStore, open_sqlite, persist, started_record, wiring_item,
};
use crate::erase::{EraseVecSink, erase_request};
use crate::erasure_reconcile::{
    ErasureReconcileReport, ErasureResume, ErasureRetryEnv, reconcile_erasure_actions,
};
use crate::erasure_retry::{
    OUTCOME_COMPLETED, OUTCOME_RECONCILED, erase_confirmed_request, erasure_action_id,
};
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::subject_request::{PeerState, SubjectRequestRecord};
use crate::subject_request_events::ProtectedAction;
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, ItemIdentity, LocalEventIntent, LocalEventState,
    LocalEventStore, SqliteStore, Store, SubjectEraseOutcome, SubjectEraseSpec,
};
use clio_types::{AmError, ErrorCode};
use clio_write::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use clio_write::{MemForest, MemTreeMaint};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-01-01T00:00:00Z";

/// A compliance-store double whose effect and key probe fault independently:
/// `fail_effect` fails `erase_subject` without marking the key destroyed, so
/// the flow sees a hard error, and `probe_error` fails the key probe.
struct FaultStore {
    fail_effect: AtomicBool,
    probe_error: AtomicBool,
    erase_calls: AtomicUsize,
}

impl FaultStore {
    fn new() -> Self {
        Self {
            fail_effect: AtomicBool::new(true),
            probe_error: AtomicBool::new(false),
            erase_calls: AtomicUsize::new(0),
        }
    }
}

impl ComplianceStore for FaultStore {
    fn list_subject_items(&self, _: &str, _: &str) -> Result<Vec<ItemIdentity>, AmError> {
        Ok(vec![])
    }

    fn erase_subject(&self, _: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        self.erase_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_effect.load(Ordering::SeqCst) {
            return Err(AmError::new(ErrorCode::Internal, "injected effect failure"));
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

    fn subject_dek_destroyed(&self, _: &str) -> Result<bool, AmError> {
        if self.probe_error.load(Ordering::SeqCst) {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected key probe failure",
            ));
        }
        Ok(false)
    }

    fn read_tombstones(&self, _: &str, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }

    fn list_tombstones(&self, _: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        Ok(vec![])
    }
}

/// Record one pending erasure intent for `request_id` over the real store.
pub(super) fn record_erasure_intent(store: &SqliteStore, request_id: &str) {
    let producer = ProtectedAction::Erasure.producer_id();
    store
        .record_intent(&LocalEventIntent {
            action_id: &erasure_action_id(request_id),
            actor: "harness",
            scope: producer,
            kind: producer,
            event_time: NOW,
        })
        .expect("the pending intent records");
}

/// The resume facts a restart supplies for one attempt's live bank set.
pub(super) fn resume_facts(attempt: &Attempt, live_banks: Vec<String>) -> ErasureResume {
    ErasureResume {
        subject_id: attempt.subject.clone(),
        live_banks,
    }
}

/// The root node's aggregate summary for one bank, or the empty string.
fn root_summary(forest: &Arc<Mutex<MemForest>>, bank: &str) -> String {
    let forest = forest.lock().expect("forest");
    let tree = forest.tree(bank).expect("tree");
    tree.get(tree.root_id().expect("root"))
        .and_then(|node| node.summary_gist.clone())
        .unwrap_or_default()
}

/// An outcome failure is reconciled after a restart without another erase —
/// even though the restart's live bank set no longer matches the confirmation.
#[test]
fn an_outcome_failure_reconciles_without_another_erase() {
    let attempt = Attempt::new();
    {
        let store = open_sqlite(&attempt.store_path());
        persist(&store, &started_record(&attempt.request_id, &attempt.scope));
        store
            .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
            .expect("seed the subject item");
    }
    let store = open_sqlite(&attempt.store_path());
    let live = live_set(&attempt, &store);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let events = FaultEvents::outcome_fails_over(&store);
    erase_confirmed_request(&store, &events, &store, &attempt.request_id, &args, &live)
        .expect_err("the failed outcome write is reported");
    let pending = store.pending_actions().expect("pending");
    assert_eq!(
        pending,
        vec![action_id(&attempt)],
        "exactly one pending action remains"
    );
    let pending_row = load_row(&store, &attempt.request_id);
    let tombstones = tombstone_count(&attempt, &store);
    drop(store);
    // The restart boundary: reopen the file-backed store before reconciling.
    let store = open_sqlite(&attempt.store_path());
    assert_eq!(
        load_row(&store, &attempt.request_id),
        pending_row,
        "the reopen leaves the pending row unchanged"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstones,
        "the reopen leaves the tombstone count unchanged"
    );
    let changed = vec![attempt.bank.clone(), unique("joined-bank")];
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, changed.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action_id(&attempt)]);
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(
        rows[0].result.as_deref(),
        Some(OUTCOME_COMPLETED),
        "the settle never claims no effect"
    );
    // A re-run would write a second tombstone, so the count pins no re-run.
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstones,
        "reconciliation repeats no key loss or store work"
    );
    let again = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile again");
    assert_eq!(again, ErasureReconcileReport::default());
}

/// A pending action with no durable record settles denied.
#[test]
fn reconcile_settles_denied_a_missing_record() {
    let (attempt, store) = persisted();
    let missing = unique("missing-request");
    record_erasure_intent(&store, &missing);
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, vec![]));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![erasure_action_id(&missing)]);
    assert!(report.pending.is_empty(), "{report:?}");
    let rows = action_rows(&store, &erasure_action_id(&missing));
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_RECONCILED));
}

/// A committed effect with an unresolved peer stays pending, never denied.
#[test]
fn reconcile_keeps_a_committed_effect_pending_while_a_peer_is_unresolved() {
    let attempt = Attempt::new();
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.peers.insert(unique("peer-bank"), PeerState::Pending);
    let store = open_sqlite(&attempt.store_path());
    persist(&store, &record);
    store
        .create_memory_item(&wiring_item(&attempt.bank, &attempt.subject), "tester")
        .expect("seed the subject item");
    let live = live_set(&attempt, &store);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    // The purge commits while the peer slot is unresolved, as when a peer joins mid-effect.
    erase_request(&args).expect("the effect commits");
    record_erasure_intent(&store, &attempt.request_id);
    assert!(tombstone_count(&attempt, &store) > 0, "the purge committed");
    let env = attempt.env(&store, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.pending, vec![action_id(&attempt)]);
    assert!(report.settled.is_empty(), "{report:?}");
    let pending = store.pending_actions().expect("pending");
    assert_eq!(pending, vec![action_id(&attempt)], "still pending");
    let rows = action_rows(&store, &action_id(&attempt));
    assert!(rows[0].result.is_none(), "no outcome is written");
}

/// No effect evidence plus a refused preflight settles the action denied.
#[test]
fn reconcile_settles_denied_when_the_preflight_refuses_without_effect_evidence() {
    let (attempt, store) = persisted();
    record_erasure_intent(&store, &attempt.request_id);
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let changed = vec![attempt.bank.clone(), unique("joined-bank")];
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, changed.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action_id(&attempt)]);
    assert!(report.pending.is_empty(), "{report:?}");
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_RECONCILED));
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the denial observed no effect"
    );
}

/// A resumed effect that reports PROCESSING stays pending, row untouched.
#[test]
fn reconcile_counts_a_processing_effect_as_pending() {
    let (attempt, store) = persisted();
    record_erasure_intent(&store, &attempt.request_id);
    let mut double = RetryStore::failing();
    double.banks = vec![attempt.bank.clone()];
    let sink = EraseVecSink::default();
    let env = attempt.env(&double, &store, &sink);
    let live = double.effect_bank_set();
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.pending, vec![action_id(&attempt)]);
    assert!(report.settled.is_empty(), "{report:?}");
    assert_eq!(double.erase_calls(), 1, "the effect ran once");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id(&attempt)],
        "the row stays pending"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert!(rows[0].result.is_none(), "no outcome is written");
}

/// Another event kind is left for its owner, never reconciled here.
#[test]
fn reconcile_leaves_another_event_kind_for_its_owner() {
    let (attempt, store) = persisted();
    let foreign = unique("restore-action");
    store
        .record_intent(&LocalEventIntent {
            action_id: &foreign,
            actor: "harness",
            scope: crate::restore_guard::RESTORE_EVENT_KIND,
            kind: crate::restore_guard::RESTORE_EVENT_KIND,
            event_time: NOW,
        })
        .expect("the foreign intent records");
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, vec![]));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report, ErasureReconcileReport::default());
    assert_eq!(store.pending_actions().expect("pending"), vec![foreign]);
}

/// The refresh-only resume detaches the erased subject's surviving identity
/// row from the live derived state under a real `MemTreeMaint`.
#[test]
fn reconcile_refresh_only_resume_detaches_the_erased_content() {
    let attempt = Attempt::new();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    let item_id = item.id.clone();
    {
        let store = open_sqlite(&attempt.store_path());
        persist(&store, &started_record(&attempt.request_id, &attempt.scope));
        store
            .create_memory_item(&item, "tester")
            .expect("seed the subject item");
    }
    // The live derived state still holds the erased leaf, as it does when the
    // process died before the refresh.
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    leaves.publish(item);
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached(&item_id, &attempt.bank));
    assert!(
        !maint.refresh_bank(&attempt.bank).is_empty(),
        "the seed refreshes"
    );
    let before = root_summary(&forest, &attempt.bank);
    assert!(before.contains("secret gist"), "{before}");
    // The effect commits but the outcome write fails, so a restart reconciles.
    let store = open_sqlite(&attempt.store_path());
    let live = live_set(&attempt, &store);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let events = FaultEvents::outcome_fails_over(&store);
    erase_confirmed_request(&store, &events, &store, &attempt.request_id, &args, &live)
        .expect_err("the outcome failure leaves the action pending");
    let env = ErasureRetryEnv {
        maint: Some(&maint),
        ..attempt.env(&store, &store, &sink)
    };
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action_id(&attempt)]);
    assert!(
        leaves.get(&attempt.bank, &item_id).is_none(),
        "the erased leaf is detached"
    );
    let after = root_summary(&forest, &attempt.bank);
    assert!(!after.contains("secret gist"), "{after}");
}

/// A hard effect error with no key loss stays pending through the flow and
/// the restart pass, and a later resume completes it with one allowed outcome.
#[test]
fn a_harmless_effect_failure_keeps_the_intent_pending_until_a_resume_completes() {
    let (attempt, store) = persisted();
    let double = FaultStore::new();
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&double, &ctx, &sink);
    let live = vec![attempt.bank.clone()];
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect_err("the hard effect error is reported");
    assert_eq!(
        err.message, "injected effect failure",
        "the original error returns"
    );
    let calls = double.erase_calls.load(Ordering::SeqCst);
    assert_eq!(calls, 1, "the effect ran once");
    let pending = store.pending_actions().expect("pending");
    assert_eq!(
        pending,
        vec![action_id(&attempt)],
        "the action stays pending"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert!(rows[0].result.is_none(), "no outcome, never a denial");
    // The restart pass counts the same hard error as pending, never denied.
    let env = attempt.env(&double, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.pending, vec![action_id(&attempt)]);
    double.fail_effect.store(false, Ordering::SeqCst);
    let recovered =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the resume completes");
    assert_eq!(recovered.status, "COMPLETED");
    let settled = action_rows(&store, &action_id(&attempt));
    assert_eq!(settled.len(), 1, "exactly one outcome");
    assert_eq!(settled[0].state, LocalEventState::Allowed);
}

/// A key probe error stays pending with no false denial.
#[test]
fn a_key_probe_error_keeps_the_intent_pending() {
    let (attempt, store) = persisted();
    let mut double = FaultStore::new();
    double.probe_error = AtomicBool::new(true);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&double, &ctx, &sink);
    let live = vec![attempt.bank.clone()];
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect_err("the key probe failure is reported");
    assert_eq!(err.code, ErrorCode::Internal);
    assert_eq!(
        double.erase_calls.load(Ordering::SeqCst),
        0,
        "no effect ran"
    );
    assert!(
        !attempt.journal.exists(),
        "a key probe error writes no denial"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
}
