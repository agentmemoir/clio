// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the local event writer on both backends: intent
//! persistence before a protected effect, refusal on a failed or repeated
//! intent, outcome completion with duplicate tolerance, pending
//! reconciliation by action id after a reopen, ordered reads across a reopen,
//! and pruning that keeps request and denial state.
//!
//! ## Owns
//! - The scratch SQLite directory fixture the file-backed reopen tests use
//! - The shared-Postgres open helper and its skip wording
//!
//! ## Does not own
//! - The writer implementation (`local_events`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - Shared-Postgres test serialization (`pg_lock`)
//!
//! ## Boundary
//!
//! Tests here break when the intent/outcome flow, ordering, reconciliation, or
//! prune behavior changes. Slot table presence is pinned by the
//! `migrate_compliance_slots_tests`.
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::local_kms;
use crate::local_events::{LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Event time the prune cutoff can never reach, so a parallel prune test on
/// the shared Postgres database cannot touch another test's rows.
const LATE_TIME: &str = "2026-12-31T00:00:00Z";

/// In-memory SQLite store (the `sqlite_window_tests` fixture shape).
fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// An intent with fixed field values; override one field per test as needed.
fn intent<'a>(action_id: &'a str, event_time: &'a str) -> LocalEventIntent<'a> {
    LocalEventIntent {
        action_id,
        actor: "tester",
        scope: "scope-opaque",
        kind: "erase_request",
        event_time,
    }
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// Action ids of a listing, sorted so no store order is assumed.
fn action_ids(events: &[LocalEventView]) -> Vec<String> {
    let mut ids: Vec<String> = events.iter().map(|e| e.action_id.clone()).collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// SQLite (in-memory)
// ---------------------------------------------------------------------------

/// The intent persists as an outcome-pending record with every field it was given.
#[test]
fn an_intent_reads_back_pending_with_its_fields() {
    let store = open_sqlite();
    want(
        store.record_intent(&intent("act-1", "2026-01-01T00:00:00Z")),
        "intent",
    );
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "one intent row");
    assert_eq!(events[0].action_id, "act-1");
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].actor, "tester");
    assert_eq!(events[0].scope, "scope-opaque");
    assert_eq!(events[0].kind, "erase_request");
    assert_eq!(events[0].event_time, "2026-01-01T00:00:00Z");
    assert_eq!(events[0].state, LocalEventState::Pending);
    assert_eq!(events[0].result, None, "no result while pending");
}

/// A repeated action id refuses the intent and writes no second row.
#[test]
fn a_repeated_action_id_refuses_the_intent() {
    let store = open_sqlite();
    want(
        store.record_intent(&intent("act-1", "2026-01-01T00:00:00Z")),
        "first intent",
    );
    let err = store
        .record_intent(&intent("act-1", "2026-01-02T00:00:00Z"))
        .expect_err("a repeated action id must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "no row added");
}

/// An intent missing a required field is refused before any write.
#[test]
fn an_intent_without_required_fields_is_refused() {
    let store = open_sqlite();
    let mut blank = intent("act-1", "2026-01-01T00:00:00Z");
    blank.actor = "  ";
    let err = store
        .record_intent(&blank)
        .expect_err("blank actor refuses");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = store.list_events().expect("list events");
    assert!(events.is_empty(), "a refused intent must write no row");
}

/// An outcome write completes the pending record with its redacted result.
#[test]
fn an_outcome_completes_the_pending_record() {
    let store = open_sqlite();
    want(
        store.record_intent(&intent("act-1", "2026-01-01T00:00:00Z")),
        "intent",
    );
    want(
        store.record_outcome("act-1", false, "erased 3 items"),
        "outcome",
    );
    let events = store.list_events().expect("list events");
    assert_eq!(events.len(), 1, "no duplicate row from the outcome write");
    assert_eq!(events[0].state, LocalEventState::Allowed);
    assert_eq!(events[0].result.as_deref(), Some("erased 3 items"));
    assert_eq!(events[0].seq, 1, "the outcome must not re-order the row");
}

/// A repeated outcome write succeeds and changes nothing.
#[test]
fn a_repeated_outcome_write_is_duplicate_tolerant() {
    let store = open_sqlite();
    want(
        store.record_intent(&intent("act-1", "2026-01-01T00:00:00Z")),
        "intent",
    );
    want(
        store.record_outcome("act-1", false, "first result"),
        "first outcome",
    );
    want(
        store.record_outcome("act-1", true, "second result"),
        "repeat outcome",
    );
    let events = store.list_events().expect("list events");
    assert_eq!(events[0].state, LocalEventState::Allowed, "state kept");
    assert_eq!(
        events[0].result.as_deref(),
        Some("first result"),
        "the first outcome wins"
    );
}

/// An outcome for an action with no intent record is refused as not found.
#[test]
fn an_outcome_without_an_intent_is_refused() {
    let store = open_sqlite();
    let err = store
        .record_outcome("no-such-action", false, "r")
        .expect_err("outcome without intent refuses");
    assert_eq!(err.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------
// SQLite (file-backed reopens)
// ---------------------------------------------------------------------------

/// A scratch directory removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir("clio-store-local-events"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A pending record with no outcome write survives a reopen and reconciles by
/// action id: the outcome failed after the protected effect, the process
/// stopped, and the durable pending record is completed afterwards.
#[test]
fn a_pending_record_survives_a_reopen_and_reconciles_by_action_id() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let store = SqliteStore::open(&path, local_kms()).expect("open before the effect");
        want(
            store.record_intent(&intent("act-pending", "2026-01-01T00:00:00Z")),
            "intent",
        );
        // The protected effect would run here; the outcome write fails and
        // the process stops before any outcome is recorded.
    }
    let store = SqliteStore::open(&path, local_kms()).expect("reopen after the effect");
    assert_eq!(
        store.pending_actions().expect("pending actions"),
        vec!["act-pending".to_owned()],
        "the pending record must stay durable"
    );
    want(
        store.record_outcome("act-pending", false, "reconciled after restart"),
        "reconcile",
    );
    assert!(
        store.pending_actions().expect("pending actions").is_empty(),
        "the reconciled action leaves the pending queue"
    );
}

/// Events keep their order positions across a reopen.
#[test]
fn events_keep_their_order_across_a_reopen() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let store = SqliteStore::open(&path, local_kms()).expect("first open");
        for (id, time) in [
            ("act-a", "2026-01-01T00:00:00Z"),
            ("act-b", "2026-01-02T00:00:00Z"),
            ("act-c", "2026-01-03T00:00:00Z"),
        ] {
            want(store.record_intent(&intent(id, time)), "intent {id}");
        }
        want(store.record_outcome("act-b", false, "done"), "outcome");
    }
    let store = SqliteStore::open(&path, local_kms()).expect("reopen");
    let events = store.list_events().expect("list events");
    let ids: Vec<_> = events.iter().map(|e| e.action_id.as_str()).collect();
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(ids, vec!["act-a", "act-b", "act-c"], "order survives");
    assert_eq!(seqs, vec![1, 2, 3]);
}

/// Pruning deletes only completed allowed rows before the cutoff: request
/// (pending) and denial rows always stay.
#[test]
fn pruning_keeps_request_and_denial_state() {
    let store = open_sqlite();
    for (id, time) in [
        ("req-old", "2026-01-01T00:00:00Z"),
        ("deny-old", "2026-01-01T00:00:00Z"),
        ("allow-old", "2026-01-01T00:00:00Z"),
        ("allow-new", "2026-01-09T00:00:00Z"),
    ] {
        want(store.record_intent(&intent(id, time)), "intent {id}");
    }
    want(
        store.record_outcome("deny-old", true, "refused by policy"),
        "denial",
    );
    want(
        store.record_outcome("allow-old", false, "erased"),
        "old allowed",
    );
    want(
        store.record_outcome("allow-new", false, "erased"),
        "new allowed",
    );
    let pruned = store
        .prune_completed_before("2026-01-05T00:00:00Z")
        .expect("prune");
    assert_eq!(pruned, 1, "only the old allowed row is pruned");
    let events = store.list_events().expect("list events");
    assert_eq!(
        action_ids(&events),
        vec![
            "allow-new".to_owned(),
            "deny-old".to_owned(),
            "req-old".to_owned()
        ],
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

// ---------------------------------------------------------------------------
// Postgres (shared database; skips when it cannot be opened)
// ---------------------------------------------------------------------------

/// The shared Postgres store, or `None` when it cannot be opened (the
/// `compliance_store_pg_export_read_tests` fixture shape).
fn open_pg() -> Option<PostgresStore> {
    let url = crate::db_url::postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms)
        .inspect_err(|err| {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres local event tests", err)
            );
        })
        .ok()
}

/// Postgres events keep their order positions across a reopen.
#[test]
fn postgres_events_keep_their_order_across_a_reopen() {
    let _guard = crate::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let tag = unique("pg-le-order");
    let id = |n: char| format!("{tag}-{n}");
    for name in ['a', 'b', 'c'] {
        let action = id(name);
        want(
            store.record_intent(&intent(&action, LATE_TIME)),
            "intent {action}",
        );
    }
    drop(store);
    let events: Vec<_> = open_pg()
        .expect("reopen the shared postgres store")
        .list_events()
        .expect("list events")
        .into_iter()
        .filter(|e| e.action_id.starts_with(&tag))
        .collect();
    let ids: Vec<_> = events.iter().map(|e| e.action_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![id('a'), id('b'), id('c')],
        "order positions survive the reopen"
    );
}

/// A Postgres pending record survives a reopen and reconciles by action id,
/// and a repeated outcome write on the reconciled row changes nothing.
#[test]
fn postgres_a_pending_record_reconciles_by_action_id_after_a_reopen() {
    let _guard = crate::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let action = unique("pg-le-pending");
    want(store.record_intent(&intent(&action, LATE_TIME)), "intent");
    drop(store);
    let store = open_pg().expect("reopen the shared postgres store");
    let pending = store.pending_actions().expect("pending actions");
    assert!(
        pending.iter().any(|id| id == &action),
        "pending must stay durable"
    );
    want(
        store.record_outcome(&action, false, "reconciled after restart"),
        "fix",
    );
    want(
        store.record_outcome(&action, true, "second try"),
        "repeat outcome",
    );
    let pending = store.pending_actions().expect("pending actions");
    assert!(
        !pending.iter().any(|id| id == &action),
        "reconciled, not pending"
    );
    let events: Vec<_> = store
        .list_events()
        .expect("list events")
        .into_iter()
        .filter(|e| e.action_id == action)
        .collect();
    assert_eq!(events.len(), 1, "no duplicate row from the outcome write");
    assert_eq!(events[0].state, LocalEventState::Allowed, "state kept");
    assert_eq!(
        events[0].result.as_deref(),
        Some("reconciled after restart"),
        "the first outcome wins"
    );
}

/// Postgres pruning deletes only completed allowed rows before the cutoff:
/// request (pending) and denial rows always stay.
#[test]
fn postgres_pruning_keeps_request_and_denial_state() {
    let _guard = crate::lock_pg_test();
    let Some(store) = open_pg() else { return };
    let tag = unique("pg-le-prune");
    let id = |n: &str| format!("{tag}-{n}");
    for (name, time) in [
        ("req-old", "2026-01-01T00:00:00Z"),
        ("deny-old", "2026-01-01T00:00:00Z"),
        ("allow-old", "2026-01-01T00:00:00Z"),
        ("allow-new", "2026-01-09T00:00:00Z"),
    ] {
        want(
            store.record_intent(&intent(&id(name), time)),
            "intent {name}",
        );
    }
    want(
        store.record_outcome(&id("deny-old"), true, "refused by policy"),
        "deny",
    );
    want(
        store.record_outcome(&id("allow-old"), false, "erased"),
        "old allowed",
    );
    want(
        store.record_outcome(&id("allow-new"), false, "erased"),
        "new allowed",
    );
    let pruned = store
        .prune_completed_before("2026-01-05T00:00:00Z")
        .expect("prune");
    // The cutoff cannot be exact on a shared database that earlier runs may
    // have left completed rows in: this run's old allowed rows must all go.
    assert!(pruned >= 1, "the old allowed row must be pruned");
    let events: Vec<_> = store
        .list_events()
        .expect("list events")
        .into_iter()
        .filter(|e| e.action_id.starts_with(&tag))
        .collect();
    let prefix = format!("{tag}-");
    let mut names: Vec<&str> = events
        .iter()
        .map(|e| e.action_id.strip_prefix(&prefix).expect("tagged id"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["allow-new", "deny-old", "req-old"],
        "request and denial rows must survive pruning"
    );
}
