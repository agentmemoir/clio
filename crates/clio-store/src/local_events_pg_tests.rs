// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the local event writer on Postgres: ordered reads across a
//! reopen, pending reconciliation by action id after a restart, and pruning
//! that keeps request and denial state. It also pins the fail-closed rule for
//! an unreachable store, so a green run proves the Postgres path actually ran.
//!
//! ## Owns
//! - The shared-Postgres open helpers (`open_pg`, `open_pg_at`) and their
//!   fail-closed skip wording
//! - The Postgres reopen, reconciliation, and prune assertions
//!
//! ## Does not own
//! - The writer implementation (`local_events`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`local_events_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the intent/outcome flow, ordering, reconciliation, or
//! prune behavior changes on Postgres, or when the fail-closed skip wording
//! changes.
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::local_events::{LocalEventIntent, LocalEventState, LocalEventStore};
use crate::postgres::PostgresStore;
use crate::test_unique::unique;
use clio_types::AmError;
use std::any::Any;
use std::sync::Arc;

/// Event time the prune cutoff can never reach, so a parallel prune test on
/// the shared Postgres database cannot touch another test's rows.
const LATE_TIME: &str = "2026-12-31T00:00:00Z";

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

/// Open the local event store at `url`, failing closed when Postgres is
/// unreachable: the open panics unless `CLIO_SKIP_PG_TESTS` opts out, in which
/// case the credential-safe skip note is printed and `None` is returned.
fn open_pg_at(url: &str) -> Option<PostgresStore> {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(url, kms)
        .inspect_err(|err| {
            crate::pg_skip::unreachable_pg_or_skip("postgres local event tests", err);
        })
        .ok()
}

/// The shared Postgres store, or `None` after the fail-closed skip line.
fn open_pg() -> Option<PostgresStore> {
    let url = crate::db_url::postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    open_pg_at(&url)
}

/// A deliberately unreachable address: nothing listens on loopback port 1, so
/// the open fails without touching the operator's configured database.
const UNREACHABLE_URL: &str = "postgres://nope:nope@127.0.0.1:1/none";

/// The text of a caught panic, whichever shape its payload takes.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// Regression cover for issue #239: the Postgres local event helper printed a
/// skip note and returned early whenever the store could not be opened, so a
/// green run proved nothing about the Postgres path. The open now fails closed:
/// an unreachable store panics with the fail-closed reason unless the operator
/// set `CLIO_SKIP_PG_TESTS`, which is the only path that returns `None`.
#[test]
fn an_unreachable_postgres_fails_closed_unless_opted_out() {
    let opened = std::panic::catch_unwind(|| open_pg_at(UNREACHABLE_URL));
    if crate::pg_skip::skip_requested() {
        let store = opened.unwrap_or_else(|_| panic!("an opted-out skip must not panic"));
        assert!(
            store.is_none(),
            "an opted-out skip returns None, not a store"
        );
        return;
    }
    match opened {
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            assert!(
                message.contains("require a reachable Postgres"),
                "the panic must name the fail-closed rule: {message}"
            );
        }
        Ok(_) => panic!("an unreachable Postgres must not open a store"),
    }
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
            &format!("intent {action}"),
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
            &format!("intent {name}"),
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
