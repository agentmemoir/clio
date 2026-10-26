// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite correction close stamps owned by
//! `sqlite_correct`: a prior row closed in the second it started keeps a
//! transaction interval that is not empty.
//!
//! ## Owns
//! - The same-second correction case and the later-stamp guard on that path
//!
//! ## Does not own
//! - Correction refusals and their rollback (`audit_model_correct_reject_tests`)
//! - The close-stamp rule itself (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what a
//! same-second correction leaves stored on the prior row should normally update
//! tests here.

use crate::audit_model::CorrectionSpec;
use crate::batch_test_support_tests::{item, store};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::MemoryItem;

const START: &str = "2026-01-01T00:00:00Z";
const WIDENED: &str = "2026-01-01T00:00:01Z";

/// One corrected copy of the prior row: only the id differs, so the subject and
/// the stamps stay acceptable to `MemoryItem::validate`.
fn corrected(prior: &MemoryItem, new_id: &str) -> MemoryItem {
    let mut new = prior.clone();
    new.id = new_id.to_owned();
    new
}

/// Run one correction of `prior_id` at `stamp` and leave its prior row closed.
fn correct(s: &SqliteStore, bank: &str, prior_id: &str, new_id: &str, stamp: &str) {
    let prior = s.get_memory_item(bank, prior_id).unwrap().unwrap();
    s.correct_item(&CorrectionSpec {
        bank_id: bank.to_owned(),
        prior_item_id: prior_id.to_owned(),
        new_item: corrected(&prior, new_id),
        reason: "same-second correction".to_owned(),
        actor: "tester".to_owned(),
        timestamp: stamp.to_owned(),
    })
    .unwrap();
}

#[test]
fn correct_in_the_priors_starting_second_stores_one_second_later() {
    let s = store();
    let bank = unique("b-correct-same-second");
    let prior = item("item-prior", &bank);
    assert_eq!(prior.tx_from, START, "fixture starts the row at {START}");
    s.create_memory_item(&prior, "tester").expect("create");

    correct(&s, &bank, "item-prior", "item-corrected", START);

    let row = s.get_memory_item(&bank, "item-prior").unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some(WIDENED),
        "the close lands one second past the row's own start"
    );
    assert!(
        row.tx_from.as_str() < row.tx_until.as_deref().unwrap(),
        "a transaction read at {} sits inside [{:?})",
        row.tx_from,
        row.tx_until
    );
    assert_eq!(
        row.valid_until.as_deref(),
        Some(START),
        "the valid axis keeps the requested stamp"
    );
}

#[test]
fn correct_after_the_priors_start_keeps_the_requested_stamp() {
    let s = store();
    let bank = unique("b-correct-later");
    let prior = item("item-prior", &bank);
    s.create_memory_item(&prior, "tester").expect("create");

    correct(
        &s,
        &bank,
        "item-prior",
        "item-corrected",
        "2026-03-01T00:00:00Z",
    );

    let row = s.get_memory_item(&bank, "item-prior").unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some("2026-03-01T00:00:00Z"),
        "a later close is stored byte for byte"
    );
    assert_eq!(row.valid_until.as_deref(), Some("2026-03-01T00:00:00Z"));
}
