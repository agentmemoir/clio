// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres compare-and-set contract test:
//! a claim/stale write made from a record the caller read earlier, and what
//! happens when the stored row has moved since.
//!
//! ## Owns
//! - The compare-and-set applied, refused, dead-unit and mismatched-pair
//!   scenarios on Postgres
//!
//! ## Does not own
//! - The unconditional claim/stale update scenarios
//!   (`postgres_distilled_unit_tests`)
//! - The SQLite originals (`distilled_unit_cas_tests`)
//! - The record shape (`distilled_unit`) and the guard's SQL
//!   (`postgres_distilled_unit_store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Changes to the Postgres compare-and-set behaviour should normally require
//! changes here. Changes to the unconditional write should require
//! `postgres_distilled_unit_tests`.
//!
//! Keep this module focused on the Postgres compare-and-set scenarios.

use crate::distilled_revision::snapshot_projection;
use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::postgres_distilled_unit::postgres_distilled_unit_tests::pg_store;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::sync::Arc;

/// A committed unit over two sources, and the ids the scenarios need.
fn committed(store: &PostgresStore, tag: &str) -> (String, String, String) {
    let bank = unique(tag);
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
    let r = record(&bank, &item_id, &hub, &[a.clone(), b.clone()]);
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    (bank, item_id, b)
}

#[test]
fn postgres_compare_and_set_applies_and_reprojects_when_the_row_still_matches() {
    let (_guard, store) = pg_store();
    let (bank, item_id, _withdrawn) = committed(&store, "pg-cas-applied");

    let expected = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    let mut updated = expected.clone();
    assert!(updated.mark_source_changed("2026-06-04T00:00:00Z"));
    assert!(
        store
            .update_distilled_unit_if_unchanged(&expected, &updated, "tester")
            .unwrap()
    );

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read.stale_reason.as_deref(), Some(STALE_SOURCE_CHANGED));
    assert_eq!(read.stale_at.as_deref(), Some("2026-06-04T00:00:00Z"));
    let snapshot = store.get_snapshot(&bank, &item_id).unwrap().unwrap();
    assert_eq!(snapshot, snapshot_projection(&read));
}

#[test]
fn postgres_compare_and_set_refuses_a_row_the_discard_cascade_moved() {
    let (_guard, store) = pg_store();
    let (bank, item_id, withdrawn) = committed(&store, "pg-cas-moved");

    let expected = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(expected.stale_reason, None);

    let mut cascaded = expected.clone();
    cascaded.source_ids.retain(|id| id != &withdrawn);
    cascaded.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    cascaded.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
    store.update_distilled_unit(&cascaded, "tester").unwrap();

    let mut updated = expected.clone();
    assert!(updated.mark_source_changed("2026-06-04T00:00:00Z"));
    let applied = store
        .update_distilled_unit_if_unchanged(&expected, &updated, "tester")
        .unwrap();
    assert!(!applied, "a row that moved must refuse the write");

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read, cascaded, "the record is the cascade's, unchanged");
    assert!(
        !read.source_ids.contains(&withdrawn),
        "a source that left active use stays out of the claim set"
    );
    assert_eq!(read.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    let snapshot = store.get_snapshot(&bank, &item_id).unwrap().unwrap();
    assert_eq!(snapshot, snapshot_projection(&cascaded));
}

#[test]
fn postgres_compare_and_set_leaves_a_dead_unit_untouched() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-cas-dead");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let r = record(&bank, &item_id, &hub, &[]);
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    // Simulate the erasure purge: content gone, record row left behind.
    let purge = "UPDATE items SET content_ciphertext = NULL WHERE id = $1 AND bank_id = $2";
    store
        .with_client(|client| {
            client
                .execute(purge, &[&item_id, &bank])
                .am("erase distilled pg")
        })
        .unwrap();

    let expected = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    let mut updated = expected.clone();
    assert!(updated.mark_source_changed("2026-06-04T00:00:00Z"));
    let applied = store
        .update_distilled_unit_if_unchanged(&expected, &updated, "tester")
        .unwrap();
    assert!(
        !applied,
        "a dead unit is left alone, not reported as a race"
    );
    assert_eq!(
        store.get_distilled_unit(&bank, &item_id).unwrap().unwrap(),
        r,
        "a dead unit's record must stay untouched"
    );
}

#[test]
fn postgres_compare_and_set_rejects_records_for_two_different_units() {
    let (_guard, store) = pg_store();
    let (bank, item_id, _withdrawn) = committed(&store, "pg-cas-mismatch");
    let expected = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    let mut updated = expected.clone();
    updated.item_id = format!("{}:other", expected.item_id);
    updated.mark_source_changed("2026-06-04T00:00:00Z");

    let err = store
        .update_distilled_unit_if_unchanged(&expected, &updated, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
}

// The guard is one `Arc`-free copy of the same SQL, so both stores answer the
// same question; this pins that the two backends agree on the refusal.
#[test]
fn both_backends_refuse_the_same_moved_row() {
    let sqlite = SqliteStore::open_in_memory(local_kms()).unwrap();
    let (_guard, postgres) = pg_store();
    let mut verdicts = Vec::new();
    for store in [
        Arc::new(sqlite) as Arc<dyn Store>,
        Arc::new(postgres) as Arc<dyn Store>,
    ] {
        let bank = unique("both-cas");
        let subject = unique("subject");
        let hub = unique("hub");
        let a = unique("source-a");
        let b = unique("source-b");
        let item_id = format!("hub-distill:{bank}:{hub}");
        let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
        let r = record(&bank, &item_id, &hub, &[a.clone(), b.clone()]);
        store.commit_distilled_unit(&item, &r, "tester").unwrap();
        let expected = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
        let mut cascaded = expected.clone();
        cascaded.source_ids.retain(|id| id != &b);
        cascaded.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
        cascaded.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
        store.update_distilled_unit(&cascaded, "tester").unwrap();
        let mut updated = expected.clone();
        updated.mark_source_changed("2026-06-04T00:00:00Z");
        verdicts.push(
            store
                .update_distilled_unit_if_unchanged(&expected, &updated, "tester")
                .unwrap(),
        );
    }
    assert_eq!(verdicts, vec![false, false], "sqlite then postgres");
}
