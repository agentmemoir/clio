// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the outcome-failure arm of the review-export
//! route: the review file lands, the outcome write that settles it fails, and
//! the restart reconcile settles the action without redelivering the file or
//! changing its bytes.
//!
//! ## Owns
//! - The review-export outcome-failure and restart-reconcile arm
//!
//! ## Does not own
//! - The shared `Fault` and `FaultStore` doubles and the intent-failure arms
//!   (see `scenario_intent_outcome_faults`)
//! - The export, erasure, restore, and Postgres outcome arms (see
//!   `scenario_intent_outcome_faults_reconcile`)
//! - The pending-restore reconcile under the intent fault (see
//!   `scenario_intent_outcome_faults_pending_restore`)
//! - The seeded stores and the scratch directory (see `case_fixtures`)
//!
//! ## Boundary
//!
//! Changes to which fault points the review-export claim proves should
//! normally require changes here. Changes to the durable store contract should
//! not.
//!
//! Keep this module focused on the review-export outcome arm.

use crate::case_fixtures::{open_sqlite, scratch};
use crate::scenario_intent_outcome_faults::{Fault, FaultStore, accepted, ctx, event_rows, spec};
use clio::ErrorCode;
use clio::store::{LocalEventState, LocalEventStore};
use clio_compliance::{
    DeliveryResult, LocalReviewQuery, deliver_export, reconcile_open_destinations,
    write_review_export,
};

/// A failed review-export outcome leaves one file effect and one pending
/// action that reconciles after a restart without redelivering.
#[test]
fn a_review_export_outcome_failure_reconciles_without_redelivery() {
    let dir = scratch("review-outcome");
    let db = dir.join("store.db");
    let (destination, staging) = (dir.join("review.txt"), dir.join("staging"));
    let query = LocalReviewQuery {
        limit: 100,
        cursor: None,
        request_id: None,
    };
    let delivered = {
        let store = open_sqlite(&db, &dir.join("kms"));
        // A settled control export, so the rendered review file carries bytes
        // the reconcile can be checked against.
        let control = dir.join("control.bin");
        assert_eq!(
            deliver_export(
                &ctx(),
                &store,
                &store,
                &accepted(),
                None,
                &spec("control-act", &control, &dir.join("control-staging")),
                b"control",
            ),
            DeliveryResult::Delivered
        );
        let events = FaultStore {
            inner: &store,
            fault: Fault::Outcome,
        };
        let err = write_review_export(
            &ctx(),
            &events,
            &store,
            &query,
            &accepted(),
            &spec("review-act", &destination, &staging),
        )
        .expect_err("the outcome fault is reported");
        assert_eq!(err.code, ErrorCode::Internal);
        let delivered = std::fs::read(&destination).expect("read the delivered review file");
        assert!(!delivered.is_empty(), "the file effect landed once");
        assert_eq!(
            event_rows(&store, "review-act")[0].state,
            LocalEventState::Pending
        );
        assert_eq!(
            store.pending_actions().expect("pending"),
            vec!["review-act".to_owned()]
        );
        delivered
    };
    let store = open_sqlite(&db, &dir.join("kms"));
    reconcile_open_destinations(&store, &store).expect("reconcile");
    let rows = event_rows(&store, "review-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    assert_eq!(
        std::fs::read(&destination).expect("read the review file"),
        delivered,
        "the reconcile never redelivers or damages the file"
    );
}
