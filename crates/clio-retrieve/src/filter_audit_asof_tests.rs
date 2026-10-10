// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the historical-query test: after a
//! discrete correction, the bi-temporal visibility predicate on the transaction
//! axis selects the original value at a past timestamp and the corrected value
//! afterwards, using the real rows the correction wrote.
//!
//! ## Owns
//! - Store-backed `item_visible_at` transaction-axis assertions
//!
//! ## Does not own
//! - Correction SQL (`clio-store`) or hybrid candidate filtering (`crate::hybrid`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Note: hybrid retrieval's lexical/dense legs intentionally index only live
//! rows (`items.tx_until IS NULL`), so `retrieve`/`compose` `as_of` filters the
//! live candidate set on the validity axis rather than resurrecting superseded
//! revisions. Transaction-axis historical reads of an item go through
//! `audit_trail`/row reads plus this visibility predicate.

use crate::filter::item_visible_at;
use crate::fixtures_tests as fx;
use clio_store::{CorrectionSpec, Store};
use clio_types::TimeAxis;
use serde_json::json;

#[test]
fn t18_04_as_of_transaction_returns_prior_then_corrected_value() {
    let store = fx::store();
    let bank = "bank-asof";
    let mut prior = fx::item("rev-1", bank, "deploy cadence nightly");
    prior.tx_from = "2026-01-01T00:00:00Z".into();
    prior.valid_from = "2026-01-01T00:00:00Z".into();
    prior.created_at = "2026-01-01T00:00:00Z".into();
    store.create_memory_item(&prior, "agent").expect("create");

    let mut corrected = prior.clone();
    corrected.id = "rev-2".into();
    corrected.gist = Some("deploy cadence weekly".into());
    corrected.snapshot = Some(json!({"cadence": "weekly"}));
    corrected.tx_from = "2026-02-01T00:00:00Z".into();
    corrected.valid_from = "2026-02-01T00:00:00Z".into();
    corrected.created_at = "2026-02-01T00:00:00Z".into();
    corrected.updated_at = "2026-02-01T00:00:00Z".into();
    store
        .correct_item(&CorrectionSpec {
            bank_id: bank.into(),
            prior_item_id: "rev-1".into(),
            new_item: corrected,
            reason: "cadence changed".into(),
            actor: "agent".into(),
            timestamp: "2026-02-01T00:00:00Z".into(),
        })
        .expect("correct");

    let past = store
        .get_memory_item(bank, "rev-1")
        .expect("prior read")
        .expect("prior exists");
    let now = store
        .get_memory_item(bank, "rev-2")
        .expect("corrected read")
        .expect("corrected exists");

    // Before the correction: the original revision is the visible past belief
    // and still carries its original value; the corrected one is not yet visible.
    assert!(item_visible_at(
        &past,
        TimeAxis::Transaction,
        "2026-01-15T00:00:00Z"
    ));
    assert!(!item_visible_at(
        &now,
        TimeAxis::Transaction,
        "2026-01-15T00:00:00Z"
    ));
    assert_eq!(past.gist.as_deref(), Some("deploy cadence nightly"));

    // After the correction: only the corrected value is visible; the prior is
    // retained but closed — it is never deleted.
    assert!(!item_visible_at(
        &past,
        TimeAxis::Transaction,
        "2026-03-01T00:00:00Z"
    ));
    assert!(item_visible_at(
        &now,
        TimeAxis::Transaction,
        "2026-03-01T00:00:00Z"
    ));
    assert_eq!(now.gist.as_deref(), Some("deploy cadence weekly"));
}
