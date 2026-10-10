// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite distilled-unit derivation-record
//! store contract tests: round-trip agreement with the snapshot, bank scope,
//! atomic rollback, validation refusals, and record deletion with the item row.
//!
//! ## Owns
//! - SQLite record round-trip/atomicity/refusal scenarios and the shared
//!   `hub_item` fixture
//!
//! ## Does not own
//! - Postgres record scenarios (`postgres_distilled_unit_tests`) or the
//!   erasure scenarios (`erase_distilled_unit_tests`)
//!
//! ## Boundary
//!
//! Changes to the SQLite record store contract should normally require changes
//! here. Changes to Postgres scenarios should require
//! `postgres_distilled_unit_tests`; changes to erasure retention should require
//! `erase_distilled_unit_tests`.
//!
//! Keep this module focused on SQLite record storage scenarios.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_unit::{DISTILLED_UNIT_RECORD_VERSION, DistilledUnitRecord};
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{ErrorCode, MemoryItem};
use rusqlite::params;
use std::collections::BTreeSet;

/// A semantic item whose snapshot projects the same claim set the record
/// carries: the shape a hub distillation writes.
pub(crate) fn hub_item(
    bank: &str,
    id: &str,
    subject: &str,
    hub: &str,
    sources: &[&str],
) -> MemoryItem {
    let mut item = memory_item(bank, id, subject);
    item.source_ref = Some("hub_distilled".into());
    item.snapshot = Some(serde_json::json!({
        "hub_id": hub,
        "neighbor_ids": sources,
        "neighbor_count": sources.len(),
    }));
    item
}

/// A well-formed record: marker and timestamp match what `hub_item` projects.
fn hub_record(bank: &str, item_id: &str, hub: &str) -> DistilledUnitRecord {
    DistilledUnitRecord::new(
        bank,
        item_id,
        hub,
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    )
}

#[test]
fn sqlite_commit_and_read_round_trip_agrees_with_the_snapshot() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
    // The repeated `a` proves `new` deduplicates while keeping order.
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        vec![a.clone(), b.clone(), a.clone()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read, record);
    assert_eq!(read.bank_id, bank);
    assert_eq!(read.item_id, item_id);
    assert_eq!(read.hub_id, hub);
    assert_eq!(read.source_ids, vec![a.clone(), b.clone()]);
    assert_eq!(read.marker, "hub_distilled");
    assert_eq!(read.record_version, DISTILLED_UNIT_RECORD_VERSION);
    assert_eq!(read.distilled_at, "2026-06-01T00:00:00Z");

    // The record is the claim-set authority; the snapshot projects the same ids.
    let snapshot = store.get_snapshot(&bank, &item_id).unwrap().unwrap();
    let mut expected: BTreeSet<String> = BTreeSet::new();
    expected.insert(snapshot["hub_id"].as_str().unwrap().to_owned());
    for id in snapshot["neighbor_ids"].as_array().unwrap() {
        expected.insert(id.as_str().unwrap().to_owned());
    }
    assert_eq!(read.claim_ids(), expected);
    assert_eq!(read.claim_ids().len(), 3);
}

#[test]
fn sqlite_get_distilled_unit_is_bank_scoped_and_missing_is_none() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-scope");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = unique("distilled");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let record = hub_record(&bank, &item_id, &hub);
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();

    assert_eq!(
        store.get_distilled_unit(&bank, &item_id).unwrap(),
        Some(record)
    );
    assert!(
        store
            .get_distilled_unit(&unique("other-bank"), &item_id)
            .unwrap()
            .is_none(),
        "another bank must not resolve the record"
    );
    assert!(
        store
            .get_distilled_unit(&bank, &unique("missing"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn sqlite_record_write_failure_leaves_no_item_behind() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-fail");
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
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO distilled_units
                    (bank_id, item_id, hub_id, source_ids_json, marker, record_version, distilled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    seeded.bank_id,
                    seeded.item_id,
                    seeded.hub_id,
                    crate::distilled_unit::encode_source_ids(&seeded.source_ids),
                    seeded.marker,
                    i64::from(seeded.record_version),
                    seeded.distilled_at,
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

    // A record that does not match the item is rejected before any write.
    let bad_item = unique("bad-item");
    let bad = DistilledUnitRecord::new(
        unique("other-bank"),
        &bad_item,
        unique("hub"),
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    let err = store
        .commit_distilled_unit(&memory_item(&bank, &bad_item, &subject), &bad, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        store.get_memory_item(&bank, &bad_item).unwrap().is_none(),
        "no row may be written for a mismatched record"
    );
    assert!(
        store
            .get_distilled_unit(&bank, &bad_item)
            .unwrap()
            .is_none()
    );
}

#[test]
fn sqlite_delete_item_removes_the_record_with_the_row() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-delete");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = unique("distilled");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let record = hub_record(&bank, &item_id, &hub);
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
fn sqlite_an_invalid_record_is_refused_before_any_write() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-invalid");
    let subject = unique("subject");
    let item_id = unique("item");
    let item = hub_item(&bank, &item_id, &subject, &unique("hub"), &[]);
    let valid = || hub_record(&bank, &item_id, &unique("hub"));

    // An empty bank is a missing bank, the same code the other store writers use.
    let mut record = valid();
    record.bank_id = String::new();
    assert_eq!(
        store
            .commit_distilled_unit(&item, &record, "tester")
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );

    // Every identity field is required: an id-keyed record without its own key
    // could never be resolved. A zero version or one past the shared
    // Postgres-integer ceiling is a shape this build cannot write on either
    // backend.
    let blanks: [fn(&mut DistilledUnitRecord); 6] = [
        |r| r.item_id = String::new(),
        |r| r.hub_id = String::new(),
        |r| r.marker = String::new(),
        |r| r.distilled_at = String::new(),
        |r| r.record_version = 0,
        |r| r.record_version = i32::MAX as u32 + 1,
    ];
    for blank in blanks {
        let mut record = valid();
        blank(&mut record);
        assert_eq!(
            store
                .commit_distilled_unit(&item, &record, "tester")
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument,
            "{record:?}"
        );
    }

    assert_refused(&store, &bank, &item_id);
}

/// A refused commit must leave neither the item row nor the record behind.
fn assert_refused(store: &SqliteStore, bank: &str, item_id: &str) {
    assert!(
        store.get_memory_item(bank, item_id).unwrap().is_none(),
        "a refused commit must leave no item behind"
    );
    assert!(
        store.get_distilled_unit(bank, item_id).unwrap().is_none(),
        "a refused commit must leave no record behind"
    );
}

/// Overwrite one column of a stored record with a corrupt value.
fn corrupt(store: &SqliteStore, bank: &str, item_id: &str, assignment: &str) {
    store
        .with_conn(|conn| {
            conn.execute(
                &format!(
                    "UPDATE distilled_units SET {assignment} WHERE bank_id = ?1 AND item_id = ?2"
                ),
                params![bank, item_id],
            )
            .am("corrupt record")?;
            Ok(())
        })
        .expect("corrupt the row");
}

#[test]
fn sqlite_a_corrupt_record_row_is_reported_not_guessed() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-corrupt");
    let subject = unique("subject");
    let item_id = unique("item");
    let item = hub_item(&bank, &item_id, &subject, &unique("hub"), &[]);
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        unique("hub"),
        Vec::new(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();

    // A malformed source list is not a claim set.
    corrupt(&store, &bank, &item_id, "source_ids_json = 'not json'");
    assert!(
        store.get_distilled_unit(&bank, &item_id).is_err(),
        "a malformed source list must be reported"
    );
    // A version outside the `u32` range is a shape this build cannot read. The
    // row passes the column's own `>= 1` check, so only the reader can refuse it
    // (the source list is put back so the version is the only fault).
    corrupt(
        &store,
        &bank,
        &item_id,
        "source_ids_json = '[]', record_version = 4294967296",
    );
    assert!(
        store.get_distilled_unit(&bank, &item_id).is_err(),
        "an out-of-range record version must be reported"
    );
}

#[test]
fn sqlite_commit_refuses_a_structurally_invalid_item() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-bad-item");
    let subject = unique("subject");
    let item_id = unique("item");
    let hub = unique("hub");
    // A semantic item without its required category fails the same structural
    // validation every other store write path performs.
    let mut item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    item.category = None;
    let record = hub_record(&bank, &item_id, &hub);
    assert_eq!(
        store
            .commit_distilled_unit(&item, &record, "tester")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_refused(&store, &bank, &item_id);
}

#[test]
fn sqlite_a_marker_contradicting_the_item_source_ref_is_refused() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-distilled-marker");
    let subject = unique("subject");
    let item_id = unique("item");
    let hub = unique("hub");
    let record = hub_record(&bank, &item_id, &hub);
    // A missing source_ref cannot carry the marker.
    let unmarked = memory_item(&bank, &item_id, &subject);
    assert_eq!(
        store
            .commit_distilled_unit(&unmarked, &record, "tester")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    // A different marker is the same mismatch: the record would claim
    // provenance the item does not carry.
    let mut other = hub_item(&bank, &item_id, &subject, &hub, &[]);
    other.source_ref = Some("something_else".into());
    assert_eq!(
        store
            .commit_distilled_unit(&other, &record, "tester")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_refused(&store, &bank, &item_id);
}
