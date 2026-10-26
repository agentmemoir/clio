// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite distilled-unit revision close
//! stamps owned by `sqlite_distilled_unit`: a prior revision closed in the
//! second it started keeps a transaction interval that is not empty.
//!
//! ## Owns
//! - The same-second revision case and the later-stamp guard on that path
//!
//! ## Does not own
//! - The rest of the revision contract (`distilled_unit_reconcile_tests`)
//! - The close-stamp rule itself (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what a
//! same-second revision leaves stored on the prior row should normally update
//! tests here.

use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::sqlite::SqliteStore;
use crate::store::Store;

const START: &str = "2026-06-01T00:00:00Z";
const WIDENED: &str = "2026-06-01T00:00:01Z";

#[test]
fn revise_in_the_priors_starting_second_stores_one_second_later() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-revise-same-second");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();
    assert_eq!(first.tx_from, START);

    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    second_record.distilled_at = START.to_owned();
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    let prior = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        prior.tx_until.as_deref(),
        Some(WIDENED),
        "the close lands one second past the prior's own start"
    );
    assert!(
        prior.tx_from.as_str() < prior.tx_until.as_deref().unwrap(),
        "a transaction read at {} sits inside [{:?})",
        prior.tx_from,
        prior.tx_until
    );
    assert_eq!(
        prior.valid_until.as_deref(),
        Some(START),
        "the valid axis keeps the distilled stamp"
    );
}

#[test]
fn revise_after_the_priors_start_keeps_the_distilled_stamp() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-revise-later");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();

    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    second_record.distilled_at = "2026-07-01T00:00:00Z".to_owned();
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    let prior = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        prior.tx_until.as_deref(),
        Some("2026-07-01T00:00:00Z"),
        "a later close is stored byte for byte"
    );
    assert_eq!(prior.valid_until.as_deref(), Some("2026-07-01T00:00:00Z"));
}
