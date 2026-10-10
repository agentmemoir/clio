// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres distilled-unit derivation-record
//! store contract tests: round-trip agreement, atomic rollback on a failed
//! record write, deletion with the item row, and the shared `record_version`
//! ceiling.
//!
//! ## Owns
//! - Postgres record round-trip/atomicity/version-ceiling scenarios
//! - Postgres revision/update/list parity scenarios (`distilled_unit_reconcile_tests`)
//!
//! ## Does not own
//! - SQLite record scenarios (`distilled_unit_tests`)
//! - The record shape (`distilled_unit`)
//!
//! ## Boundary
//!
//! Changes to the Postgres record store contract, or to the SQLite reconcile
//! scenarios mirrored here, should normally require changes here. Changes to
//! SQLite-only scenarios should not require changes here.
//!
//! Keep this module focused on Postgres record storage scenarios.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_revision::snapshot_projection;
use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{DistilledUnitRecord, STALE_SOURCE_REMOVED, encode_source_ids};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::collections::BTreeSet;

#[test]
fn postgres_commit_and_read_round_trip_is_bank_scoped() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-distilled");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        vec![a.clone()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read, record);
    assert_eq!(
        read.claim_ids(),
        [hub, a].into_iter().collect::<BTreeSet<_>>()
    );
    assert!(
        store
            .get_distilled_unit(&unique("pg-other-bank"), &item_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_distilled_unit(&bank, &unique("pg-missing"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn postgres_record_write_failure_leaves_no_item_behind() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-distilled-fail");
    let subject = unique("subject");
    let item_id = unique("item");
    let seeded = DistilledUnitRecord::new(
        &bank,
        &item_id,
        unique("hub"),
        vec![unique("source")],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .with_client(|client| {
            client
                .execute(
                    "INSERT INTO distilled_units
                        (bank_id, item_id, hub_id, source_ids_json, marker, record_version,
                         distilled_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    &[
                        &seeded.bank_id,
                        &seeded.item_id,
                        &seeded.hub_id,
                        &encode_source_ids(&seeded.source_ids),
                        &seeded.marker,
                        &i32::try_from(seeded.record_version).unwrap(),
                        &seeded.distilled_at,
                    ],
                )
                .am("seed distilled unit")?;
            Ok(())
        })
        .unwrap();

    // The item insert succeeds, the record insert collides, and the one
    // transaction rolls both back. The item's provenance agrees with the
    // record's marker so the commit reaches the record-write collision.
    let hub = unique("hub");
    let source = unique("source");
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        vec![source.clone()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&source]);
    assert!(
        store
            .commit_distilled_unit(&item, &record, "tester")
            .is_err(),
        "a duplicate record key must refuse the commit"
    );
    assert!(
        store.get_memory_item(&bank, &item_id).unwrap().is_none(),
        "the item insert rolled back with the record conflict"
    );
    assert_eq!(
        store.get_distilled_unit(&bank, &item_id).unwrap().unwrap(),
        seeded,
        "the pre-seeded record is unchanged"
    );
}

#[test]
fn postgres_delete_item_removes_the_record_with_the_row() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-distilled-delete");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = unique("distilled");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();

    assert!(store.delete_item(&bank, &item_id).unwrap());
    assert!(store.get_memory_item(&bank, &item_id).unwrap().is_none());
    assert!(
        store.get_distilled_unit(&bank, &item_id).unwrap().is_none(),
        "the record is deleted whenever the item row is"
    );
}

#[test]
fn postgres_an_out_of_range_record_version_is_refused_before_any_write() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-distilled-version");
    let subject = unique("subject");
    let item_id = unique("item");
    let hub = unique("hub");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let mut record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    record.record_version = i32::MAX as u32 + 1;
    assert_eq!(
        store
            .commit_distilled_unit(&item, &record, "tester")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument,
        "both backends must share the same record_version ceiling"
    );
    assert!(store.get_memory_item(&bank, &item_id).unwrap().is_none());
    assert!(store.get_distilled_unit(&bank, &item_id).unwrap().is_none());
}

#[test]
fn postgres_insert_tx_still_refuses_an_out_of_range_record_version() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-distilled-guard");
    let item_id = unique("item");
    let hub = unique("hub");
    let mut record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    record.record_version = i32::MAX as u32 + 1;
    // The shared validation refuses the commit path first; this belt-and-braces
    // guard keeps a caller that skips it from binding past the column range.
    store
        .with_client(|client| {
            let mut tx = client.transaction().am("insert guard tx begin")?;
            let err = crate::postgres_distilled_unit::insert_tx(&mut tx, &record).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidArgument);
            tx.rollback().am("insert guard tx rollback")?;
            Ok(())
        })
        .unwrap();
}

/// A store plus the shared-Postgres lock guard, which must outlive the test.
pub(crate) fn pg_store() -> (crate::PgTestGuard, PostgresStore) {
    let guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    (guard, store)
}

/// Commits a distilled unit for `sources` and returns its item and record.
fn commit_unit(
    store: &PostgresStore,
    bank: &str,
    subject: &str,
    hub: &str,
    sources: &[String],
) -> (clio_types::MemoryItem, DistilledUnitRecord) {
    let item_id = format!("hub-distill:{bank}:{hub}");
    let refs: Vec<&str> = sources.iter().map(String::as_str).collect();
    let item = hub_item(bank, &item_id, subject, hub, &refs);
    let r = record(bank, &item_id, hub, sources);
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    (item, r)
}

/// Raw lineage-column read for one item row (the columns are DB-only).
fn lineage_columns(store: &PostgresStore, item_id: &str) -> (Option<String>, Option<String>) {
    store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT superseded_by_id, corrects_item_id FROM items WHERE id = $1",
                    &[&item_id],
                )
                .am("read distilled lineage columns")?;
            Ok((row.get(0), row.get(1)))
        })
        .unwrap()
}

#[test]
fn postgres_list_distilled_units_is_bank_scoped_and_ordered_by_unit_id() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-reconcile-list");
    let subject = unique("subject");
    let mut ids = Vec::new();
    for hub in ["h2", "h1"] {
        let (_, r) = commit_unit(&store, &bank, &subject, hub, &[]);
        ids.push(r.item_id);
    }
    ids.sort();
    let listed = store.list_distilled_units(&bank).unwrap();
    let got: Vec<String> = listed.iter().map(|r| r.item_id.clone()).collect();
    assert_eq!(got, ids, "the list is ordered by unit id");
    let other = store.list_distilled_units(&unique("other-bank")).unwrap();
    assert!(other.is_empty(), "{other:?}");
}

#[test]
fn postgres_revise_closes_the_prior_on_both_axes_and_keeps_it() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-reconcile-revise");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (first, _) = commit_unit(&store, &bank, &subject, &hub, std::slice::from_ref(&a));

    let b = unique("source-b");
    let mut second_record = record(&bank, &first.id, &hub, std::slice::from_ref(&b));
    second_record.revision = 2;
    second_record.distilled_at = "2026-06-02T00:00:00Z".to_owned();
    let second_id = second_record.current_revision_item_id();
    let second = hub_item(&bank, &second_id, &subject, &hub, &[&b]);
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    // The prior revision is closed on both axes, retained, and linked.
    let prior = store.get_memory_item(&bank, &first.id).unwrap().unwrap();
    assert_eq!(prior.gist, first.gist, "the prior text is retained");
    assert_eq!(prior.valid_until.as_deref(), Some("2026-06-02T00:00:00Z"));
    assert_eq!(prior.tx_until.as_deref(), Some("2026-06-02T00:00:00Z"));
    let superseded = lineage_columns(&store, &first.id).0;
    assert_eq!(superseded, Some(second.id.clone()));

    // The current revision resolves through the record, not the unit id.
    let read = store.get_distilled_unit(&bank, &first.id).unwrap().unwrap();
    assert_eq!(read, second_record);
    assert_eq!(read.revision, 2, "the record moved to the successor");
    let current_id = read.current_revision_item_id();
    let current = store.get_memory_item(&bank, &current_id).unwrap().unwrap();
    assert_eq!(current.id, format!("{}:v2", first.id));
    let corrects = lineage_columns(&store, &current.id).1;
    assert_eq!(corrects, Some(first.id.clone()));
    assert_eq!(current.tx_until, None);

    // Record and snapshot agree after the revision.
    let snapshot = store.get_snapshot(&bank, &current_id).unwrap().unwrap();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    claimed.insert(snapshot["hub_id"].as_str().unwrap().to_owned());
    for id in snapshot["neighbor_ids"].as_array().unwrap() {
        claimed.insert(id.as_str().unwrap().to_owned());
    }
    assert_eq!(read.claim_ids(), claimed);
}

#[test]
fn postgres_revise_refuses_a_wrong_revision_id_or_a_closed_prior() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-reconcile-refuse");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (first, _) = commit_unit(&store, &bank, &subject, &hub, std::slice::from_ref(&a));

    // A wrong revision item id is refused before any write.
    let mut second_record = record(&bank, &first.id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    let wrong_id = hub_item(&bank, &unique("wrong-id"), &subject, &hub, &[&a]);
    let err = store
        .revise_distilled_unit(&second_record, &wrong_id, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        store
            .get_memory_item(&bank, &unique("wrong-id"))
            .unwrap()
            .is_none(),
        "no item may be written when the revision is refused",
    );

    let leaked = store.get_memory_item(&bank, &unique("wrong-id")).unwrap();
    assert!(leaked.is_none(), "no item was written for the refusal");

    // The honest revision closes the prior...
    let second_id = second_record.current_revision_item_id();
    let second = hub_item(&bank, &second_id, &subject, &hub, &[&a]);
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    // ...and a second revision over the closed prior is refused: the record
    // still says revision 2, so the derived prior id is the closed row.
    let third_record = second_record.clone();
    let third_id = third_record.current_revision_item_id();
    let third = hub_item(&bank, &third_id, &subject, &hub, &[&a]);
    let err = store
        .revise_distilled_unit(&third_record, &third, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn postgres_update_distilled_unit_drops_the_claim_and_marks_stale() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-reconcile-update");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (item, r) = commit_unit(&store, &bank, &subject, &hub, std::slice::from_ref(&a));

    let mut updated = r.clone();
    updated.source_ids = Vec::new();
    updated.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    updated.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
    store.update_distilled_unit(&updated, "tester").unwrap();

    let read = store.get_distilled_unit(&bank, &item.id).unwrap().unwrap();
    assert_eq!(read.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(read.stale_at.as_deref(), Some("2026-06-03T00:00:00Z"));
    assert_eq!(read.source_ids, Vec::<String>::new());
    assert_eq!(read.revision, 1, "no revision happened");
    assert_eq!(read.distilled_at, r.distilled_at, "not re-distilled");

    // The current item's snapshot was re-projected from the same record state.
    let snapshot = store.get_snapshot(&bank, &item.id).unwrap().unwrap();
    assert_eq!(snapshot, snapshot_projection(&read));
    // The gist half is untouched by the claim update.
    let gist = store.get_gist(&bank, &item.id).unwrap().unwrap();
    assert_eq!(gist, item.gist.unwrap());
}

#[test]
fn postgres_update_distilled_unit_leaves_an_erased_unit_untouched() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-reconcile-erased");
    let subject = unique("subject");
    let hub = unique("hub");
    let (item, r) = commit_unit(&store, &bank, &subject, &hub, &[]);
    // Simulate the erasure purge: content gone, record row left behind.
    let purge = "UPDATE items SET content_ciphertext = NULL WHERE id = $1 AND bank_id = $2";
    store
        .with_client(|client| {
            client
                .execute(purge, &[&item.id, &bank])
                .am("erase distilled content pg")
        })
        .unwrap();

    let mut updated = r.clone();
    updated.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    updated.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
    store.update_distilled_unit(&updated, "tester").unwrap();
    let read = store.get_distilled_unit(&bank, &item.id).unwrap().unwrap();
    assert_eq!(read, r, "a dead unit's record must stay untouched");
}
