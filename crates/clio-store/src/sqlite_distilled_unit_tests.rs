// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite distilled-unit revision and read
//! paths owned by `sqlite_distilled_unit`: the same-second revision close
//! stamp, the bank-scoped record read and list, and a revision whose prior is
//! already closed.
//!
//! ## Owns
//! - The revision and read/list paths of that module
//!
//! ## Does not own
//! - The rest of the revision contract (`distilled_unit_reconcile_tests`)
//! - The close-stamp rule itself (`migrate_canonical_tests`)
//! - Update and cascade paths (`sqlite_distilled_unit_update_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what a
//! same-second revision leaves stored on the prior row should normally update
//! tests here.

use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, ErrorCode};
use rusqlite::ToSql;

const START: &str = "2026-06-01T00:00:00Z";
const WIDENED: &str = "2026-06-01T00:00:01Z";

/// One raw statement against the store's own connection, for the column states
/// no write path mints (an unreadable body, a closed revision, a missing row).
pub(crate) fn exec(store: &SqliteStore, sql: &str, args: &[&dyn ToSql]) {
    store
        .with_conn(|conn| {
            conn.execute(sql, args)
                .map(|_| ())
                .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap();
}

pub(crate) fn seed_source(store: &SqliteStore, bank: &str, subject: &str, id: &str) {
    let item = memory_item(bank, id, subject);
    store.create_memory_item(&item, "tester").unwrap();
}

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

#[test]
fn a_revision_whose_prior_is_already_closed_is_refused() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-revise-closed");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    store
        .commit_distilled_unit(
            &first,
            &record(&bank, &item_id, &hub, std::slice::from_ref(&a)),
            "tester",
        )
        .unwrap();
    // An operator correction closed the prior row outside reconciliation.
    exec(
        &store,
        "UPDATE items SET tx_until = ?1 WHERE id = ?2 AND bank_id = ?3",
        &[&"2026-07-01T00:00:00Z", &item_id, &bank],
    );

    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    let err = store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        store
            .get_memory_item(&bank, &second_record.current_revision_item_id())
            .unwrap()
            .is_none(),
        "the refused revision inserts nothing"
    );
}

#[test]
fn a_record_reads_back_by_its_id_and_lists_in_unit_order() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-read");
    let subject = unique("subject");
    let mut ids = Vec::new();
    for hub in ["hub-1", "hub-2"] {
        let item_id = format!("hub-distill:{bank}:{hub}");
        let item = hub_item(&bank, &item_id, &subject, hub, &[]);
        store
            .commit_distilled_unit(&item, &record(&bank, &item_id, hub, &[]), "tester")
            .unwrap();
        ids.push(item_id);
    }

    let one = store.get_distilled_unit(&bank, &ids[1]).unwrap().unwrap();
    assert_eq!(one.item_id, ids[1]);
    assert_eq!(one.marker, "hub_distilled");
    assert_eq!(one.revision, 1);
    assert_eq!(one.distilled_at, START);
    assert!(store.get_distilled_unit(&bank, &ids[0]).unwrap().is_some());

    let listed: Vec<String> = store
        .list_distilled_units(&bank)
        .unwrap()
        .into_iter()
        .map(|r| r.item_id)
        .collect();
    assert_eq!(listed, ids, "one bank's records, ordered by unit id");

    let other = unique("b-other");
    assert!(store.get_distilled_unit(&other, &ids[0]).unwrap().is_none());
    assert_eq!(store.list_distilled_units(&other).unwrap().len(), 0);
}
