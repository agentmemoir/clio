// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconcile states the sibling suites have
//! no room for: a destroyed key whose purge never committed and stays pending
//! until a later pass runs the purge, a reconcile denial that is terminal for
//! the flow, and a resume whose subject disagrees with the durable record.
//!
//! ## Owns
//! - The partial-effect, terminal-denial, and foreign-subject assertions
//!
//! ## Does not own
//! - The reconcile (`erasure_reconcile.rs`), the flow (`erasure_retry.rs`), and
//!   the shared fixtures (`erasure_retry_tests.rs`,
//!   `erasure_retry_reconcile_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when reconciliation settles over uncommitted store work,
//! when a denied action is resumed, or when a resume may probe a subject its
//! record does not name.
//!
//! Test-only. Never compiled into production builds.

use super::erasure_retry_reconcile_tests::{record_erasure_intent, resume_facts};
use super::erasure_retry_tests::{
    action_id, action_rows, live_set, load_row, persisted, tombstone_count,
};
use super::subject_request_store_wiring_tests::{RetryStore, wiring_item};
use crate::erase::EraseVecSink;
use crate::erasure_reconcile::{ErasureResume, reconcile_erasure_actions};
use crate::erasure_retry::{OUTCOME_COMPLETED, OUTCOME_RECONCILED, erase_confirmed_request};
use crate::subject_request::SubjectRequestRecord;
use crate::test_unique::unique;
use clio_store::{ComplianceStore, LocalEventState, LocalEventStore, Store};
use clio_types::ErrorCode;
use std::sync::atomic::Ordering;

/// A destroyed key with an uncommitted purge stays pending across passes, and
/// is settled by the pass that finally commits the purge.
#[test]
fn reconcile_keeps_a_destroyed_key_without_the_purge_pending() {
    let (attempt, store) = persisted();
    record_erasure_intent(&store, &attempt.request_id);
    let mut double = RetryStore::failing();
    double.banks = vec![attempt.bank.clone()];
    let sink = EraseVecSink::default();
    let env = attempt.env(&double, &sink);
    let live = double.effect_bank_set();
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, live.clone()));
    let first = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(first.pending, vec![action_id(&attempt)]);
    assert!(first.settled.is_empty(), "{first:?}");
    assert!(
        double.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the first pass destroyed the key without committing the purge"
    );
    assert_eq!(
        double.erase_calls(),
        1,
        "the first pass runs the effect once"
    );
    // The second pass starts from a destroyed key and an uncommitted purge: it
    // must retry the purge instead of settling a completion over it.
    let second = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(second.pending, vec![action_id(&attempt)], "still pending");
    assert!(second.settled.is_empty(), "{second:?}");
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(double.erase_calls(), 2, "the second pass retries the purge");
    double.fail.store(false, Ordering::SeqCst);
    let third = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(
        third.settled,
        vec![action_id(&attempt)],
        "the pass that commits the purge settles"
    );
    let settled = action_rows(&store, &action_id(&attempt));
    assert_eq!(settled.len(), 1, "exactly one outcome");
    assert_eq!(settled[0].state, LocalEventState::Allowed);
    assert_eq!(settled[0].result.as_deref(), Some(OUTCOME_COMPLETED));
    assert_eq!(double.erase_calls(), 3, "each pass runs the purge once");
    assert!(
        double.subject_dek_destroyed(&attempt.subject).expect("key"),
        "the completed pass recreates no key"
    );
}

/// A reconcile denial is terminal: a later flow call for the same request
/// refuses with no store effect, no key loss, and no completion over the row.
#[test]
fn reconcile_denial_refuses_a_later_flow_call() {
    let (attempt, store) = persisted();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    record_erasure_intent(&store, &attempt.request_id);
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &sink);
    let changed = vec![attempt.bank.clone(), unique("joined-bank")];
    let resume = |_: &SubjectRequestRecord| Some(resume_facts(&attempt, changed.clone()));
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action_id(&attempt)]);
    let denied = action_rows(&store, &action_id(&attempt));
    assert_eq!(denied[0].state, LocalEventState::Denied);
    assert_eq!(denied[0].result.as_deref(), Some(OUTCOME_RECONCILED));
    let before = load_row(&store, &attempt.request_id);
    let ctx = attempt.ctx();
    let live = live_set(&attempt, &store);
    let args = attempt.args(&store, &ctx, &sink);
    // The changed bank set is cleared up, so only the denial can refuse here.
    let err = erase_confirmed_request(&store, &store, &attempt.request_id, &args, &live)
        .expect_err("a denied action is never resumed");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "the denial is never overwritten");
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(
        rows[0].result.as_deref(),
        Some(OUTCOME_RECONCILED),
        "no completion is written over the denial"
    );
    assert_eq!(
        load_row(&store, &attempt.request_id),
        before,
        "the refusal leaves the durable row untouched"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the denial settled the action"
    );
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
        "the subject's item survives the refusal"
    );
}

/// A resume naming another subject refuses before any probe, so neither the
/// other subject's rows nor the record's own rows are touched.
#[test]
fn reconcile_refuses_a_resume_that_names_another_subject() {
    let (attempt, store) = persisted();
    let other_subject = unique("other-subject");
    let mine = wiring_item(&attempt.bank, &attempt.subject);
    let theirs = wiring_item(&attempt.bank, &other_subject);
    store
        .create_memory_item(&mine, "tester")
        .expect("seed the record's subject");
    store
        .create_memory_item(&theirs, "tester")
        .expect("seed the other subject");
    record_erasure_intent(&store, &attempt.request_id);
    let sink = EraseVecSink::default();
    let env = attempt.env(&store, &sink);
    let live = live_set(&attempt, &store);
    let resume = |_: &SubjectRequestRecord| {
        Some(ErasureResume {
            subject_id: other_subject.clone(),
            live_banks: live.clone(),
        })
    };
    let err = reconcile_erasure_actions(&store, &store, &env, &resume)
        .expect_err("a foreign-subject resume must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    let rows = action_rows(&store, &action_id(&attempt));
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Pending, "never settled");
    assert!(rows[0].result.is_none(), "no outcome is written");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id(&attempt)],
        "the action stays pending for a correct resume"
    );
    assert!(
        store
            .get_memory_item(&attempt.bank, &theirs.id)
            .expect("readable")
            .is_some(),
        "the other subject's rows stay untouched"
    );
    assert!(
        store
            .get_memory_item(&attempt.bank, &mine.id)
            .expect("readable")
            .is_some(),
        "no effect ran for the record's own subject either"
    );
    assert!(
        !store.subject_dek_destroyed(&other_subject).expect("key"),
        "the refusal loses no key"
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        0,
        "the refusal writes no tombstone"
    );
}
