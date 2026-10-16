// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the completion-guard coverage of the
//! confirmed-erasure retry entry and its restart resume: the denied
//! racing-intent winner, the failed journal denial a later pass retries, the
//! committed purge evidence bound to the resumed subject, and the copy-action
//! arm of the settle gate; the actor refusals live in
//! `erasure_retry_completion_guard_split_tests.rs`.
//!
//! ## Owns
//! - The denied-race, journal-failure, purge-binding, and copy-arm assertions
//!   over the retry entry and reconciliation
//! - The denied-winner event-writer double
//!
//! ## Does not own
//! - The flow (`erasure_retry.rs`), the reconcile (`erasure_reconcile.rs`),
//!   and the shared fixtures (`erasure_retry_tests.rs`,
//!   `erasure_retry_reconcile_tests.rs`,
//!   `subject_request_store_wiring_tests.rs`)
//! - The actor refusals (`erasure_retry_completion_guard_split_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when a foreign-subject call reaches the durable intent or
//! the effect, when a denied winner is resumed, when a failed denial is not
//! retried by a later pass, or when the settle gate opens over an open copy
//! action.
//!
//! Test-only. Never compiled into production builds.

use super::erasure_retry_reconcile_tests::{record_erasure_intent, resume_facts};
use super::erasure_retry_tests::{action_id, action_rows, live_set, persisted, tombstone_count};
use super::subject_request_store_wiring_tests::{
    Scratch, kms, persist, started_record, wiring_item,
};
use crate::erase::{EraseArgs, EraseVecSink, erase_request};
use crate::erasure_reconcile::{ErasureResume, ErasureRetryEnv, reconcile_erasure_actions};
use crate::erasure_retry::{
    OUTCOME_COMPLETED, OUTCOME_RECONCILED, erase_confirmed_request, erasure_action_id,
};
use crate::inventory_fixture::FixtureDir;
use crate::subject_request::{
    RequestAction, RequestActionKind, RequestActionState, RequestScope, SubjectRequestRecord,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    SqliteStore, Store,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};

const NOW: &str = "2026-01-01T00:00:00Z";

/// A bank-scoped resume and retry whose committed purge names another subject
/// stay pending and settle nothing; the resume naming the purged subject
/// settles.
#[test]
#[allow(clippy::too_many_lines)]
fn a_bank_scoped_resume_does_not_settle_over_another_subjects_purge() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let bank = unique("wiring-bank");
    let purged_subject = unique("subject");
    let other_subject = unique("subject");
    let scope = RequestScope::Bank {
        bank_id: bank.clone(),
    };
    let journal_dir = FixtureDir::create(&unique("erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");
    // A fresh salt per test; retries reuse it and no assertion fixes a hash.
    let salt = rand::random::<[u8; 16]>();
    let other_item = wiring_item(&bank, &other_subject);
    let kms = kms();
    let store = SqliteStore::open(&path, kms.clone()).expect("scratch sqlite opens");
    persist(&store, &started_record(&request_id, &scope));
    store
        .create_memory_item(&wiring_item(&bank, &purged_subject), "tester")
        .expect("seed the purged subject's item");
    store
        .create_memory_item(&other_item, "tester")
        .expect("seed the other subject's item");
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Harness);
    let live = store
        .erase_bank_set(&bank, &purged_subject)
        .expect("live bank set");
    let args = EraseArgs {
        store: &store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: &purged_subject,
        legal_basis: "GDPR_ART_17",
        request_id: &request_id,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: NOW,
        sink: &sink,
    };
    erase_request(&args).expect("the purged subject's effect commits");
    record_erasure_intent(&store, &request_id);
    let tombstones = store
        .read_tombstones(&bank, &request_id)
        .expect("tombstones");
    assert!(!tombstones.is_empty(), "the purge leaves evidence");
    let env = ErasureRetryEnv {
        store: &store,
        peers: &store,
        journal_path: &journal,
        maint: None,
        actor: Actor::Harness,
        legal_basis: "GDPR_ART_17",
        salt: &salt,
        timestamp: NOW,
        sink: &sink,
    };
    let action = erasure_action_id(&request_id);
    // The other subject has no tombstone under this request: settling here
    // would report a completion for work that never ran for it.
    let resume_other = |_: &SubjectRequestRecord| {
        Some(ErasureResume {
            subject_id: other_subject.clone(),
            live_banks: live.clone(),
        })
    };
    let report = reconcile_erasure_actions(&store, &store, &env, &resume_other).expect("reconcile");
    assert_eq!(report.pending, vec![action.clone()]);
    assert!(report.settled.is_empty(), "{report:?}");
    let rows = action_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert!(
        !store
            .subject_dek_destroyed(&other_subject)
            .expect("probe the other subject's key"),
        "the other subject's key survives"
    );
    assert!(
        store
            .get_memory_item(&bank, &other_item.id)
            .expect("readable")
            .is_some(),
        "the other subject's data survives"
    );
    let after = store
        .read_tombstones(&bank, &request_id)
        .expect("tombstones");
    assert_eq!(after.len(), tombstones.len(), "no new work");
    // The reviewer's edge: a direct key loss leaves the resumed subject's key
    // already destroyed while this request's tombstones name the purged
    // subject; the retry entry must stay pending with no effect for it.
    kms.destroy_dek(&other_subject).expect("the key loss");
    let retry = EraseArgs {
        subject_id: &other_subject,
        ..args
    };
    let outcome = erase_confirmed_request(&store, &store, &store, &request_id, &retry, &live)
        .expect("the retry entry reports the pending action");
    assert_eq!(outcome.status, "PROCESSING", "pending action");
    let rows = action_rows(&store, &action);
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    let after = store
        .read_tombstones(&bank, &request_id)
        .expect("tombstones");
    assert_eq!(after.len(), tombstones.len(), "no effect runs for it");
    // The resume naming the purged subject follows the legitimate path.
    let resume_purged = |_: &SubjectRequestRecord| {
        Some(ErasureResume {
            subject_id: purged_subject.clone(),
            live_banks: live.clone(),
        })
    };
    let settled =
        reconcile_erasure_actions(&store, &store, &env, &resume_purged).expect("reconcile");
    assert_eq!(settled.settled, vec![action.clone()]);
    assert!(settled.pending.is_empty(), "{settled:?}");
    let rows = action_rows(&store, &action);
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
}

/// A racing intent winner that writes the intent, denies it, then fails the
/// duplicate write the loser sees.
struct DeniedRacingIntent<'a> {
    inner: &'a dyn LocalEventStore,
}

impl LocalEventStore for DeniedRacingIntent<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        self.inner.record_intent(intent)?;
        self.inner
            .record_outcome(intent.action_id, true, OUTCOME_RECONCILED)?;
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            "injected duplicate intent",
        ))
    }

    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        self.inner.record_outcome(action_id, denied, result)
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

/// A duplicate-intent race whose winner denied the row refuses like the direct
/// path: the denial is terminal and no effect runs.
#[test]
fn a_denied_racing_intent_winner_refuses_the_retry() {
    let (attempt, store) = persisted();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let events = DeniedRacingIntent { inner: &store };
    let err = erase_confirmed_request(&store, &events, &store, &attempt.request_id, &args, &live)
        .expect_err("a denied racing winner must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "the winner's row is the only row");
    assert_eq!(rows[0].state, LocalEventState::Denied, "never overwritten");
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_RECONCILED));
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "a denied action is settled"
    );
    assert!(!attempt.journal.exists(), "the refusal precedes the denial");
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the refusal loses no key"
    );
    assert_eq!(tombstone_count(&attempt, &store), 0, "no store effect ran");
    assert!(
        store
            .get_memory_item(&attempt.bank, &item.id)
            .expect("readable")
            .is_some(),
        "the subject's data survives"
    );
}

/// A failed journal denial refuses the retry entry with the intent already
/// durable: no key loss, no tombstone, and a later reconcile pass retries it.
#[test]
fn a_failed_journal_denial_refuses_and_a_later_pass_retries_it() {
    let (attempt, store) = persisted();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    // The journal's parent directory is deterministically absent, so the
    // denial write cannot create its temp sibling.
    let dir = FixtureDir::create(&unique("erase-journal-unwritable"));
    let unwritable = dir.path().join("absent").join("erasure-journal.json");
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let live = live_set(&attempt, &store);
    let args = EraseArgs {
        journal_path: &unwritable,
        ..attempt.args(&store, &ctx, &sink)
    };
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect_err("an unwritable journal must refuse");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        sink.events().is_empty(),
        "no telemetry for a refused denial"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id(&attempt)],
        "the intent stays pending for a later pass"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the refusal loses no key"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        0,
        "the refusal runs no store effect"
    );
    assert!(
        store
            .get_memory_item(&attempt.bank, &item.id)
            .expect("readable")
            .is_some(),
        "the subject's item survives"
    );
    // A later reconcile pass over a writable journal retries and completes it.
    let env = attempt.env(&store, &store, &sink);
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action_id(&attempt)]);
    assert!(report.pending.is_empty(), "{report:?}");
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the retry destroys the key"
    );
    assert!(
        tombstone_count(&attempt, &store) > 0,
        "the retry commits the purge"
    );
}

/// An open copy action keeps the completed effect pending with no outcome;
/// closing it lets a retry settle the same action.
#[test]
fn an_open_copy_action_keeps_the_settle_pending_until_it_closes() {
    let (attempt, store) = persisted();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    let mut record = started_record(&attempt.request_id, &attempt.scope);
    record.actions.insert(
        "copy-1".into(),
        RequestAction {
            kind: RequestActionKind::Copy,
            state: RequestActionState::Open,
        },
    );
    persist(&store, &record);
    let ctx = attempt.ctx();
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let outcome =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the effect completes");
    assert_eq!(
        outcome.status, "PROCESSING",
        "an open copy action keeps completion pending"
    );
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id(&attempt)],
        "the action stays pending"
    );
    let tombstoned = tombstone_count(&attempt, &store);
    assert!(tombstoned > 0, "the open action's effect committed");
    record.actions.get_mut("copy-1").expect("copy action").state = RequestActionState::Closed;
    persist(&store, &record);
    let retried =
        erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
            .expect("the retry settles");
    assert_eq!(retried.status, "COMPLETED");
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert_eq!(
        tombstone_count(&attempt, &store),
        tombstoned,
        "the settle repeats no destructive work"
    );
}

// Declared here, beside the fixtures it reuses: the privileged-actor refusal
// coverage of the retry entry.
#[cfg(test)]
#[path = "erasure_retry_completion_guard_split_tests.rs"]
mod erasure_retry_completion_guard_split_tests;
