// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the maintenance handoff tests: the in-process
//! leaf index and the verdict a bounded wait on the detached hook returns.
//!
//! ## Owns
//! - Leaf publish, lookup, listing order, and removal
//! - `DispatchJob::drain` for a panicking hook and for one that outlasts its budget
//!
//! ## Does not own
//! - The dispatcher and the hook contract under test
//! - What a durable hook writes (`memtree_hook_tests`)
//!
//! ## Boundary
//!
//! Changes to what a drain wait reports should normally require changes here.
//! Changes to the rows a hook writes should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::maintenance::{
    BufferMaintenance, LeafIndex, MaintenanceHook, MaintenanceSignal, NoopMaintenance,
    dispatch_async,
};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn item(id: &str, created: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: created.into(),
        updated_at: created.into(),
        context: None,
    }
}

#[test]
fn leaf_get_list_and_new() {
    let idx = LeafIndex::new();
    idx.publish(item("i2", "t0"));
    idx.publish(item("i1", "t0"));
    idx.publish(item("i0", "t1"));
    assert!(idx.get("b", "i1").is_some());
    assert!(idx.get("b", "missing").is_none());
    let listed = idx.list_leaves("b");
    assert_eq!(listed.len(), 3);
    // same created_at → id tie-break; then later created_at
    assert_eq!(listed[0].id, "i1");
    assert_eq!(listed[1].id, "i2");
    assert_eq!(listed[2].id, "i0");
    let other = idx.list_leaves("other");
    assert!(other.is_empty(), "leaves: {other:?}");
    let _ = LeafIndex::default();
}

#[test]
fn buffer_and_noop_hooks() {
    let hook = BufferMaintenance::default();
    hook.on_leaf_attached(MaintenanceSignal::leaf_attached("i1", "b"));
    assert_eq!(hook.signals().len(), 1);
    let noop = NoopMaintenance;
    noop.on_leaf_attached(MaintenanceSignal::leaf_attached("i2", "b"));
}

/// A hook that panics, so the dispatcher drops its sender before it reports.
#[derive(Debug)]
struct PanicHook;

impl MaintenanceHook for PanicHook {
    fn on_leaf_attached(&self, _signal: MaintenanceSignal) {
        panic!("hook refuses the signal");
    }
}

/// A hook that holds the signal until the test releases it.
#[derive(Debug, Default)]
struct BlockingHook {
    released: Mutex<bool>,
    ready: Condvar,
}

impl BlockingHook {
    fn release(&self) {
        *self.released.lock().expect("release flag") = true;
        self.ready.notify_all();
    }
}

impl MaintenanceHook for BlockingHook {
    fn on_leaf_attached(&self, _signal: MaintenanceSignal) {
        let mut released = self.released.lock().expect("release flag");
        while !*released {
            // The timeout only keeps a hook the test never releases from
            // blocking its thread forever; the drain's own budget is the bound
            // this test measures.
            let (guard, timed_out) = self
                .ready
                .wait_timeout(released, Duration::from_secs(10))
                .expect("wait for release");
            released = guard;
            if timed_out.timed_out() {
                return;
            }
        }
    }
}

#[test]
fn drain_reports_a_panicking_hook_as_undelivered() {
    let job = dispatch_async(
        Arc::new(PanicHook),
        vec![MaintenanceSignal::leaf_attached("i1", "b")],
    )
    .expect("a signal starts the dispatcher");

    let started = Instant::now();
    assert!(
        !job.drain(Duration::from_secs(5)),
        "a hook that panicked never reported delivery"
    );
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(1),
        "the verdict came with the panic, not when the budget ran out: {waited:?}"
    );
}

#[test]
fn drain_stops_at_the_budget_when_the_hook_never_finishes() {
    let hook = Arc::new(BlockingHook::default());
    let job = dispatch_async(
        Arc::clone(&hook) as Arc<dyn MaintenanceHook + Send + Sync>,
        vec![MaintenanceSignal::leaf_attached("i1", "b")],
    )
    .expect("a signal starts the dispatcher");

    let started = Instant::now();
    let delivered = job.drain(Duration::from_millis(20));
    let waited = started.elapsed();
    hook.release();

    assert!(!delivered, "an unfinished hook proves no delivery");
    assert!(
        waited < Duration::from_secs(1),
        "the wait ended at its budget, not when the hook finished: {waited:?}"
    );
}
