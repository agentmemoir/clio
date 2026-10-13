// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the local protected-action probe end to end: an intent
//! failure refuses with zero effects, an outcome failure after the effect
//! leaves exactly one durable pending action id that reconciles by action id
//! after a reopen, a repeated action id refuses the second effect, and
//! pruning over the probe's rows keeps request and denial state.
//!
//! ## Owns
//! - The injected-fault store stubs (failing intent write, failing outcome
//!   write over a real durable store)
//! - The scratch database path fixture and the counting effect
//!
//! ## Does not own
//! - The probe (`local_action_probe`) and the writer (`clio-store`)
//! - The store-layer intent/outcome/prune tests (`clio-store` owns those)
//!
//! ## Boundary
//!
//! Tests here break when the probe's write sequence, its outcome mapping, or
//! the writer's pending and prune behavior changes.
//!
//! Test-only. Never compiled into production builds.

use crate::local_action_probe::{LocalActionProbe, ProbeAction, ProbeOutcome};
use clio_store::{
    DekProvider, LocalDevKms, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    SqliteStore,
};
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Event time the prune cutoff can never reach.
const OLD_TIME: &str = "2026-01-01T00:00:00Z";

/// In-memory SQLite store (the `local_events_tests` fixture shape).
fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// A key provider for a file-backed store.
fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

/// A probe action with fixed field values; override one field per test.
fn action<'a>(action_id: &'a str, event_time: &'a str) -> ProbeAction<'a> {
    ProbeAction {
        action_id,
        actor: "tester",
        scope: "scope-opaque",
        event_time,
    }
}

/// An effect that counts its invocations and succeeds.
fn counting_effect(counter: &Arc<AtomicUsize>) -> impl FnOnce() -> Result<String, String> + '_ {
    let counter = Arc::clone(counter);
    move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok("probe effect ran".to_owned())
    }
}

/// A store whose intent write always fails: the injected intent fault.
struct IntentWriteFails;

impl LocalEventStore for IntentWriteFails {
    fn record_intent(&self, _intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected intent-write failure",
        ))
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::NotFound, "no intent can exist"))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        Ok(Vec::new())
    }

    fn prune_completed_before(&self, _cutoff: &str) -> Result<usize, AmError> {
        Ok(0)
    }
}

/// A store that delegates everything except the outcome write, which fails:
/// the injected outcome fault over a real durable store.
struct OutcomeWriteFails {
    inner: Arc<dyn LocalEventStore>,
}

impl LocalEventStore for OutcomeWriteFails {
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
            "injected outcome-write failure",
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

/// A scratch database path removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(crate::test_unique::unique(
            "clio-compliance-local-action-probe",
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An intent-write failure refuses the action before any effect runs.
#[test]
fn an_intent_write_failure_refuses_with_zero_effects() {
    let effects = Arc::new(AtomicUsize::new(0));
    let probe = LocalActionProbe::new(Arc::new(IntentWriteFails));
    let outcome = probe.run(&action("act-1", OLD_TIME), counting_effect(&effects));
    let ProbeOutcome::Refused { cause } = outcome else {
        panic!("intent failure must refuse: {outcome:?}");
    };
    assert_eq!(cause.code, ErrorCode::Internal, "the injected intent fault");
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "a refused action must run no effect"
    );
}

/// An outcome-write failure runs the effect once and leaves exactly one
/// durable pending action id, which reconciles by action id after a reopen.
#[test]
fn an_outcome_failure_leaves_one_pending_action_id_that_reconciles() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let effects = Arc::new(AtomicUsize::new(0));
    {
        let store = SqliteStore::open(&path, kms()).expect("open before the effect");
        let probe = LocalActionProbe::new(Arc::new(OutcomeWriteFails {
            inner: Arc::new(store),
        }));
        let outcome = probe.run(&action("act-pending", OLD_TIME), counting_effect(&effects));
        let ProbeOutcome::Pending { cause } = outcome else {
            panic!("outcome failure must stay pending: {outcome:?}");
        };
        assert_eq!(
            cause.code,
            ErrorCode::Internal,
            "the injected outcome fault"
        );
        assert_eq!(
            effects.load(Ordering::SeqCst),
            1,
            "the effect must run exactly once"
        );
    }
    let store = SqliteStore::open(&path, kms()).expect("reopen after the effect");
    assert_eq!(
        store.pending_actions().expect("pending actions"),
        vec!["act-pending".to_owned()],
        "exactly one durable pending action id"
    );
    store
        .record_outcome("act-pending", false, "reconciled after restart")
        .expect("reconcile by action id");
    assert!(
        store.pending_actions().expect("pending actions").is_empty(),
        "the reconciled action leaves the pending queue"
    );
}

/// A repeated action id refuses the second run with no new effect.
#[test]
fn a_repeated_action_id_refuses_the_second_run() {
    let probe = LocalActionProbe::new(Arc::new(open_sqlite()));
    let effects = Arc::new(AtomicUsize::new(0));
    let first = probe.run(&action("act-1", OLD_TIME), counting_effect(&effects));
    assert!(
        matches!(first, ProbeOutcome::Completed),
        "first run: {first:?}"
    );
    let second = probe.run(&action("act-1", OLD_TIME), counting_effect(&effects));
    assert!(
        matches!(second, ProbeOutcome::Refused { .. }),
        "a repeated action id must refuse: {second:?}"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the refused run must not reach the effect"
    );
}

/// Pruning over the probe's rows deletes only the completed allowed row:
/// the request (pending) and denial rows stay.
#[test]
fn pruning_keeps_request_and_denial_state() {
    let store: Arc<dyn LocalEventStore> = Arc::new(open_sqlite());
    let effects = Arc::new(AtomicUsize::new(0));
    let allowed = LocalActionProbe::new(Arc::clone(&store));
    let outcome = allowed.run(&action("allow-old", OLD_TIME), counting_effect(&effects));
    assert!(
        matches!(outcome, ProbeOutcome::Completed),
        "allow: {outcome:?}"
    );
    let denied = LocalActionProbe::new(Arc::clone(&store));
    let outcome = denied.run(&action("deny-old", OLD_TIME), || {
        Err("refused by policy".to_owned())
    });
    assert!(
        matches!(outcome, ProbeOutcome::Completed),
        "deny: {outcome:?}"
    );
    let pending = LocalActionProbe::new(Arc::new(OutcomeWriteFails {
        inner: Arc::clone(&store),
    }));
    let outcome = pending.run(&action("req-old", OLD_TIME), counting_effect(&effects));
    assert!(
        matches!(outcome, ProbeOutcome::Pending { .. }),
        "req: {outcome:?}"
    );
    let pruned = store
        .prune_completed_before("2026-01-05T00:00:00Z")
        .expect("prune");
    assert_eq!(pruned, 1, "only the completed allowed row is pruned");
    let events = store.list_events().expect("list events");
    let mut ids: Vec<String> = events.iter().map(|e| e.action_id.clone()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["deny-old".to_owned(), "req-old".to_owned()],
        "request and denial rows must survive pruning"
    );
    let state_of = |id: &str| {
        events
            .iter()
            .find(|e| e.action_id == id)
            .map(|e| e.state)
            .expect("row present")
    };
    assert_eq!(state_of("req-old"), LocalEventState::Pending);
    assert_eq!(state_of("deny-old"), LocalEventState::Denied);
}
