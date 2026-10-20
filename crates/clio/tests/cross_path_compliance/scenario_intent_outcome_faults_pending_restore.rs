// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore arm in which a pending restore
//! action is reconciled while the injected intent fault is active: the
//! reconcile writes an outcome before any new intent is written, the injected
//! fault refuses that outcome write, and the restore refuses with it.
//!
//! ## Owns
//! - The pending-restore reconcile under the injected intent fault
//!
//! ## Does not own
//! - The shared `Fault` and `FaultStore` doubles and the intent-failure arms
//!   (see `scenario_intent_outcome_faults`)
//! - The outcome-failure and restart-reconcile arms (see
//!   `scenario_intent_outcome_faults_reconcile` and
//!   `scenario_intent_outcome_faults_review_outcome`)
//! - The seeded stores and the scratch directory (see `case_fixtures`)
//!
//! ## Boundary
//!
//! Changes to which fault point the pending-restore reconcile proves should
//! normally require changes here. Changes to the durable store contract should
//! not.
//!
//! Keep this module focused on the pending-restore reconcile under the intent
//! fault.

use crate::case_fixtures::{NOW, open_sqlite, scratch};
use crate::scenario_intent_outcome_faults::{
    Fault, FaultStore, INJECTED_INTENT_OUTCOME_FAILURE, event_rows, restore_claim, restore_request,
};
use clio::ErrorCode;
use clio::store::{LocalEventIntent, LocalEventState, LocalEventStore};
use clio_compliance::{
    RESTORE_EVENT_KIND, RESTORE_EVENT_SCOPE, RestoreGuardRefusal, begin_restore, record_denial,
};

/// A pending restore action is reconciled before the new intent is written, so
/// the injected intent fault refuses the reconcile's outcome write and the
/// restore refuses with it, leaving the pending action unsettled.
#[test]
fn a_pending_restore_reconcile_refuses_before_the_new_intent_is_written() {
    let dir = scratch("restore-pending-reconcile");
    let journal = dir.join("journal.json");
    record_denial(&journal, "denied-elsewhere", &[]).expect("seed the journal");
    let claim = restore_claim(&journal);
    let store = open_sqlite(&dir.join("events.db"), &dir.join("kms"));
    store
        .record_intent(&LocalEventIntent {
            action_id: "pending-restore-act",
            actor: "harness",
            scope: RESTORE_EVENT_SCOPE,
            kind: RESTORE_EVENT_KIND,
            event_time: NOW,
        })
        .expect("seed the pending restore action");
    let events = FaultStore {
        inner: &store,
        fault: Fault::Intent,
    };
    let Err(RestoreGuardRefusal::Event(refusal)) =
        begin_restore(&events, &restore_request(&journal, &claim))
    else {
        panic!("the refused reconcile must refuse the restore");
    };
    assert_eq!(refusal.code, ErrorCode::Internal);
    assert_eq!(refusal.message, INJECTED_INTENT_OUTCOME_FAILURE);
    assert!(
        event_rows(&store, "restore-act").is_empty(),
        "the refused restore writes no intent of its own"
    );
    let pending = event_rows(&store, "pending-restore-act");
    assert_eq!(pending.len(), 1, "the seeded action is still there");
    assert_eq!(
        pending[0].state,
        LocalEventState::Pending,
        "the refused outcome left the action pending"
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec!["pending-restore-act".to_owned()],
        "the refused outcome leaves the action in the queue"
    );
}
