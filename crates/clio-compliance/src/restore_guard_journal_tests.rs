// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the journal the guarded
//! restore resolves and locks: the one shared resolution the restore CLI and
//! the erase boundary use, and the reconcile `begin_restore` runs under the
//! journal lock before it records its own intent.
//!
//! ## Owns
//! - The document-path and deployment-environment resolution assertions
//! - The under-the-lock reconcile assertion
//!
//! ## Does not own
//! - The guarded restore sequence itself (see `restore_guard.rs`)
//! - The journal authority rules (see `erasure_journal.rs`)
//! - The durable event writer (`clio-store` owns it)
//!
//! ## Boundary
//!
//! Tests here break when the journal resolution or the reconcile ordering
//! changes.
//!
//! Test-only. Never compiled into production builds.

use super::restore_guard_tests::{
    NOW, Scratch, claim_for, denied_journal, open_sqlite, restore_rows,
};
use super::{
    JournalHold, RESTORE_EVENT_KIND, RESTORE_EVENT_SCOPE, RestoreRequest, begin_restore,
    reconcile_restore_actions,
};
use crate::erasure_journal::MemberFacts;
use clio_store::{LocalEventIntent, LocalEventState, LocalEventStore};
use clio_types::ErrorCode;
use std::path::PathBuf;

#[test]
fn resolve_journal_path_accepts_a_document_equal_to_the_deployment_resolution() {
    // A document that names the same path the deployment resolves to is the one
    // accepted configuration: both the restore CLI and the erase boundary take
    // the same lock.
    let document = PathBuf::from("/tmp/host-named/journal.json");
    let get = |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV)
            .then(|| document.to_string_lossy().into_owned())
    };
    assert_eq!(
        JournalHold::resolve_journal_path(Some(&document), &get).expect("an equal document"),
        document
    );
}

#[test]
fn resolve_journal_path_refuses_a_document_divergent_from_the_environment() {
    // A host document naming a journal other than the deployment resolution
    // would let erase denial lock one file while restore placement locks
    // another, so it is refused rather than honoured.
    let document = PathBuf::from("/tmp/host-named/journal.json");
    let get = |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| "/env/journal.json".to_owned())
    };
    let error = JournalHold::resolve_journal_path(Some(&document), &get)
        .expect_err("a divergent document is refused");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error
            .message
            .contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
        "the refusal must name the remedy: {}",
        error.message
    );
}

#[test]
fn resolve_journal_path_falls_back_to_the_deployment_environment() {
    let get = |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| "/etc/clio/journal.json".to_owned())
    };
    assert_eq!(
        JournalHold::resolve_journal_path(None, &get).expect("an environment path"),
        PathBuf::from("/etc/clio/journal.json")
    );
}

#[test]
fn begin_restore_reconciles_a_pending_restore_intent_under_the_lock() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let events = open_sqlite();
    // A crashed restore left its intent pending.
    let crashed = LocalEventIntent {
        action_id: "act-crashed",
        actor: "actor",
        scope: RESTORE_EVENT_SCOPE,
        kind: RESTORE_EVENT_KIND,
        event_time: NOW,
    };
    events
        .record_intent(&crashed)
        .expect("record the crashed intent");
    // The next restore begins: it takes the lock, reconciles the pending action
    // under it, then records its own intent and re-checks the authority.
    let request = RestoreRequest {
        journal_path: &path,
        presented: None,
        claim: &claim_for(&journal),
        members: &[MemberFacts::None],
        actor: "actor",
        action_id: "act-next",
        now: NOW,
    };
    let guard = begin_restore(&events, &request).expect("the next restore begins");
    let reconciled = restore_rows(&events)
        .into_iter()
        .find(|row| row.action_id == "act-crashed")
        .expect("the crashed action has a row");
    assert_eq!(reconciled.state, LocalEventState::Denied);
    assert_eq!(
        reconciled.result.as_deref(),
        Some("reconciled after restart without repeating placement")
    );
    guard.settle(&events, true, "placed").expect("settle");
}

#[test]
fn reconcile_settles_a_pending_restore_intent_without_repeating_placement() {
    let events = open_sqlite();
    let intent = LocalEventIntent {
        action_id: "act-pending",
        actor: "actor",
        scope: RESTORE_EVENT_SCOPE,
        kind: RESTORE_EVENT_KIND,
        event_time: NOW,
    };
    events.record_intent(&intent).expect("record the intent");
    let first = reconcile_restore_actions(&events).expect("reconcile");
    assert_eq!(first, vec!["act-pending".to_owned()]);
    let rows = restore_rows(&events);
    assert_eq!(
        rows.len(),
        1,
        "the pending action becomes one terminal event"
    );
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(
        rows[0].result.as_deref(),
        Some("reconciled after restart without repeating placement")
    );
    let pending = events.pending_actions().expect("pending");
    assert!(pending.is_empty(), "no pending action remains");
    let second = reconcile_restore_actions(&events).expect("reconcile again");
    assert!(second.is_empty(), "a re-run finds nothing pending");
}

#[test]
fn reconcile_leaves_a_pending_action_of_another_kind_for_its_owner() {
    let events = open_sqlite();
    let intent = LocalEventIntent {
        action_id: "act-other",
        actor: "actor",
        scope: "export_custody",
        kind: "export_delivery",
        event_time: NOW,
    };
    events.record_intent(&intent).expect("record the intent");
    let reconciled = reconcile_restore_actions(&events).expect("reconcile");
    assert!(
        reconciled.is_empty(),
        "the other-kind action is left for its owner"
    );
    assert_eq!(
        events.pending_actions().expect("pending"),
        vec!["act-other".to_owned()]
    );
}
