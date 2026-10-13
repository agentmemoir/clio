// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the consumer-side coverage of the
//! action-event contract against the durable local event store: one recorded
//! event maps onto one `local_events` row carrying the same position, actor,
//! and kind.
//!
//! ## Owns
//! - The contract-to-store mapping these tests exercise, and its kind label
//! - The scratch database directory fixture
//!
//! ## Does not own
//! - The action-event contract and its envelope validation (see
//!   `subject_request_events.rs` and `action_event_log.rs`)
//! - The durable local event writer and its SQL (see `clio-store`)
//!
//! ## Boundary
//!
//! Tests here break when the fields an action event carries, or the store's
//! intent persistence, stop agreeing on position, actor, or kind.
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request_events::{ActionEventKind, ProtectedAction, SubjectActionEvent};
use crate::subject_request_events_tests::event;
use crate::test_unique::unique;
use clio_store::{DekProvider, LocalDevKms, LocalEventIntent, LocalEventStore, SqliteStore};
use clio_types::Actor;
use std::sync::Arc;

/// The store's kind label for one event kind: the contract's `snake_case`
/// spelling.
fn kind_label(kind: ActionEventKind) -> &'static str {
    match kind {
        ActionEventKind::Intent => "intent",
        ActionEventKind::Outcome => "outcome",
    }
}

/// Map one action event onto the store's intent record: the action id, the
/// actor and kind spellings, the scope the store keeps opaque, and the event
/// time.
fn store_intent(source: &SubjectActionEvent) -> LocalEventIntent<'_> {
    LocalEventIntent {
        action_id: source.payload.intent.action_id.as_str(),
        actor: source.actor.as_str(),
        scope: source.correlation_id.as_str(),
        kind: kind_label(source.payload.kind),
        event_time: source.time.as_str(),
    }
}

/// A scratch database directory removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(unique("clio-compliance-subject-request-events-store")))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A key provider for the file-backed scratch store.
fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}

/// One action payload maps onto exactly one durable row whose position,
/// actor, and kind are the payload's, and the row survives a reopen: the
/// single-writer correspondence the contract assumes.
#[test]
fn one_action_payload_maps_to_one_durable_row_with_the_same_position_actor_and_kind() {
    let mut source = event("e1", ProtectedAction::CopyExport, "r1");
    source.actor = Actor::Harness;

    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    {
        let store = SqliteStore::open(&path, kms()).expect("scratch sqlite opens");
        store
            .record_intent(&store_intent(&source))
            .expect("intent records");
    }
    let store = SqliteStore::open(&path, kms()).expect("reopen the scratch store");
    let rows = store.list_events().expect("list events");
    assert_eq!(rows.len(), 1, "one payload maps to one durable row");
    let row = &rows[0];
    assert_eq!(
        u64::try_from(row.seq).expect("row seq is a position"),
        source.sequence,
        "the payload's position must be the row's seq"
    );
    assert_eq!(row.actor, "harness", "the payload's actor");
    assert_eq!(row.kind, "intent", "the payload's kind");
}
