// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for measuring the guarded restore's durability
//! and recovery through its public API: a failed durable intent that places
//! nothing, a failed outcome after a permitted placement that leaves exactly
//! one pending action, and a crash before the outcome that a reopened store
//! reconciles exactly once without repeating placement.
//!
//! ## Owns
//! - The intent-failure, outcome-failure, crash-recovery, and rerun scenarios
//! - The store reopen that models a process restart between placement and
//!   reconcile
//!
//! ## Does not own
//! - The guard sequence (`clio-compliance` owns `begin_restore`)
//! - The fixtures (see `restore_guard_fixtures`), the locked-check refusals
//!   (see `restore_guard`), or the killed-child crash (see `restore_guard_crash`)
//!
//! ## Boundary
//!
//! Changes to which recovery facts are measured should normally require
//! changes here. Changes to the guard sequence should not.
//!
//! Keep this module focused on the guarded restore's durability and recovery.

use crate::restore_guard_fixtures::{
    Fault, claim_of, event_for, events_store, place, request, target,
};
use crate::support::{BANK, Scratch, denied_hash, file_sha256, journal_path};
use clio::store::{LocalEventState, LocalEventStore};
use clio_compliance::{
    MemberFacts, RestoreGuardRefusal, begin_restore, reconcile_restore_actions, record_denial,
};

#[test]
fn a_failed_intent_records_nothing_and_places_nothing() {
    let scratch = Scratch::new("guard-api-intent");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);
    let store = events_store(&scratch, "events");
    let (target_db, _kms) = target(&scratch, "target");
    let target_before = file_sha256(&target_db);

    let fault = Fault::intent_fails(&store);
    let Err(refusal) = begin_restore(
        &fault,
        &request(
            &journal,
            &claim,
            &[MemberFacts::None],
            None,
            "restore-intent",
        ),
    ) else {
        panic!("a failed intent refuses");
    };
    assert!(
        matches!(refusal, RestoreGuardRefusal::Event(_)),
        "the refusal names the failed intent: {refusal:?}"
    );
    assert!(
        store.list_events().expect("list").is_empty(),
        "a failed intent records no durable row"
    );
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "a failed intent places nothing"
    );
    println!("REFUSED guard-api failed-intent events=0 placed=false");
}

#[test]
fn a_failed_outcome_leaves_one_pending_action_that_reconciles_once() {
    let scratch = Scratch::new("guard-api-outcome");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);
    let store = events_store(&scratch, "events");
    let (target_db, _kms) = target(&scratch, "target");

    // The permitted restore: the intent is durable, the lock is held, the
    // authority passes, and placement runs.
    let fault = Fault::outcome_fails(&store);
    let guard = begin_restore(
        &fault,
        &request(
            &journal,
            &claim,
            &[MemberFacts::None],
            None,
            "restore-outcome",
        ),
    )
    .expect("the intent and authority pass");
    place(&target_db);
    let placed = file_sha256(&target_db);
    // The outcome write fails after placement.
    guard
        .settle(&fault, true, "restored")
        .expect_err("the injected outcome write fails");
    drop(store);

    // Restart: reopen the durable store before reconciling, so the pending row
    // is proven durable across a handle drop rather than read back in process.
    let store = events_store(&scratch, "events");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec!["restore-outcome".to_owned()],
        "exactly one durable pending action remains after the reopen"
    );

    // Reconcile the pending action with the real store.
    let reconciled = reconcile_restore_actions(&store).expect("reconcile after restart");
    assert_eq!(
        reconciled,
        vec!["restore-outcome".to_owned()],
        "the restart closes exactly the one pending restore action"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the pending action is closed"
    );
    let events = event_for(&store, "restore-outcome");
    assert_eq!(events.len(), 1, "one event row for the action");
    assert_eq!(events[0].state, LocalEventState::Denied);
    assert_eq!(
        file_sha256(&target_db),
        placed,
        "the reconcile placed no second time"
    );
    println!("RECONCILED guard-api failed-outcome actions=1 target_sha256={placed}");
}

#[test]
fn a_crash_before_settle_leaves_one_pending_action_that_a_reopened_store_reconciles_once() {
    let scratch = Scratch::new("guard-api-crash");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);
    let store = events_store(&scratch, "events");
    let (target_db, _kms) = target(&scratch, "target");

    // A permitted placement whose settle never runs: dropping the guard and the
    // store handle models a crash after the effect and before the outcome.
    let guard = begin_restore(
        &store,
        &request(
            &journal,
            &claim,
            &[MemberFacts::None],
            None,
            "restore-crash",
        ),
    )
    .expect("the intent and authority pass");
    place(&target_db);
    let placed = file_sha256(&target_db);
    drop(guard);
    drop(store);

    // Restart: reopen the durable event store before serving, so the pending
    // row is proven durable across a handle drop and a fresh open.
    let store = events_store(&scratch, "events");
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec!["restore-crash".to_owned()],
        "the crash left exactly one durable pending action"
    );

    // Reconcile closes the pending action without repeating placement.
    let reconciled = reconcile_restore_actions(&store).expect("reconcile after restart");
    assert_eq!(reconciled, vec!["restore-crash".to_owned()]);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the pending action is closed after reconcile"
    );
    let events = event_for(&store, "restore-crash");
    assert_eq!(events.len(), 1, "one terminal event after reconcile");
    assert_eq!(events[0].state, LocalEventState::Denied);
    assert_eq!(
        events[0].result.as_deref(),
        Some("reconciled after restart without repeating placement"),
        "the terminal event records the reconciliation"
    );
    assert_eq!(
        file_sha256(&target_db),
        placed,
        "recovery placed no second time"
    );
    println!("RECONCILED guard-api crash-recovery actions=1 target_sha256={placed}");
}

#[test]
fn a_reconciled_action_id_cannot_place_a_second_time() {
    let scratch = Scratch::new("guard-api-rerun");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);
    let store = events_store(&scratch, "events");
    let (target_db, _kms) = target(&scratch, "target");

    let guard = begin_restore(
        &store,
        &request(
            &journal,
            &claim,
            &[MemberFacts::None],
            None,
            "restore-rerun",
        ),
    )
    .expect("the intent and authority pass");
    place(&target_db);
    let placed = file_sha256(&target_db);
    drop(guard);
    drop(store);

    let store = events_store(&scratch, "events");
    let reconciled = reconcile_restore_actions(&store).expect("reconcile after restart");
    assert_eq!(reconciled, vec!["restore-rerun".to_owned()]);

    // The recovered store still denies the reused action id, so a rerun cannot
    // place a second time. The refusal is the durable-intent step: the action
    // id already carries a completed row.
    let again = begin_restore(
        &store,
        &request(
            &journal,
            &claim,
            &[MemberFacts::None],
            None,
            "restore-rerun",
        ),
    );
    let Err(refusal) = again else {
        panic!("a reconciled action id must not be reusable");
    };
    assert!(
        matches!(refusal, RestoreGuardRefusal::Event(_)),
        "the reused action id is refused at the durable-intent step: {refusal:?}"
    );
    assert_eq!(
        file_sha256(&target_db),
        placed,
        "the refused rerun placed no bytes"
    );
    println!("REFUSED guard-api rerun action=restore-rerun target_sha256={placed}");
}
