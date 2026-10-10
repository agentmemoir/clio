// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the compliance-erasure retention tests of a
//! real distilled unit: the derived item is purged, its id-keyed record follows
//! the item row's metadata retention, and the hub's own subject key survives.
//!
//! ## Owns
//! - SQLite and Postgres erase-retention scenarios for distilled units
//!
//! ## Does not own
//! - The erasure transaction (`sqlite_erase` / `postgres_erase`) or the record
//!   store contract (`distilled_unit_tests`)
//!
//! ## Boundary
//!
//! Changes to record retention across an erasure should normally require
//! changes here. Changes to the record store contract should require
//! `distilled_unit_tests`.
//!
//! Keep this module focused on erasure retention of a distilled unit.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::distilled_evidence::evidence_from_items;
use crate::distilled_unit::DistilledUnitRecord;
use crate::erase_test_support::{link_hub, local_kms, memory_item, spec, unique};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use std::collections::BTreeSet;

/// A real distilled unit: a semantic item with the `hub_distilled` marker whose
/// snapshot claims `item_a`, its id-keyed derivation record, and the explicit
/// `hub_distill` edge from the distilled item to `item_a` (the shape
/// `link_hub` writes).
fn seed_distilled_unit(
    store: &dyn Store,
    bank: &str,
    item_a: &str,
) -> (String, String, DistilledUnitRecord) {
    let hub = unique("hub");
    let hub_subject = unique("hub-subject");
    let distilled = format!("hub-distill:{bank}:{hub}");
    let mut item = memory_item(bank, &distilled, &hub_subject);
    item.source_ref = Some("hub_distilled".into());
    item.snapshot = Some(serde_json::json!({
        "hub_id": hub,
        "neighbor_ids": [item_a],
        "neighbor_count": 1,
    }));
    let record = DistilledUnitRecord::new(
        bank,
        &distilled,
        &hub,
        [item_a.to_owned()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();
    link_hub(store, bank, &distilled, item_a);
    (distilled, hub_subject, record)
}

/// The record is retained with the item row it belongs to, claim set intact.
fn assert_record_retained(
    store: &dyn Store,
    bank: &str,
    distilled: &str,
    item_a: &str,
    record: &DistilledUnitRecord,
) {
    let retained = store
        .get_distilled_unit(bank, distilled)
        .unwrap()
        .expect("the record is retained with the item row");
    assert_eq!(&retained, record);
    let expected: BTreeSet<String> = [record.hub_id.clone(), item_a.to_owned()]
        .into_iter()
        .collect();
    assert_eq!(retained.claim_ids(), expected);
}

#[test]
fn sqlite_erase_purges_a_distilled_unit_and_retains_its_record() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-erase-distilled");
    let subject = unique("subject");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject), "tester")
        .unwrap();
    let (distilled, hub_subject, record) = seed_distilled_unit(&store, &bank, &item_a);

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.derived_items_purged, 1);
    assert!(
        store.get_snapshot(&bank, &distilled).is_err(),
        "the purged quote is unreadable"
    );
    assert_record_retained(&store, &bank, &distilled, &item_a, &record);
    assert!(
        !store.subject_dek_destroyed(&hub_subject).unwrap(),
        "the hub's own subject DEK is untouched"
    );
}

/// A revised distilled unit: the unit's current revision is a second item row
/// (`{unit}:v2`) with its own cross-reference edge, both claiming `item_a`,
/// and the record advanced to revision 2. The engine writes the revision's
/// cross-reference edge; here `link_hub` stands in for it.
fn seed_revised_distilled_unit(
    store: &dyn Store,
    bank: &str,
    item_a: &str,
) -> (String, String, String, DistilledUnitRecord) {
    let (distilled, hub_subject, first_record) = seed_distilled_unit(store, bank, item_a);
    let mut second_record = first_record.clone();
    second_record.revision = 2;
    second_record.distilled_at = "2026-06-02T00:00:00Z".to_owned();
    let rev2 = second_record.current_revision_item_id();
    let mut item = memory_item(bank, &rev2, &hub_subject);
    item.source_ref = Some("hub_distilled".into());
    item.snapshot = Some(serde_json::json!({
        "hub_id": second_record.hub_id,
        "neighbor_ids": [item_a],
        "neighbor_count": 1,
    }));
    store
        .revise_distilled_unit(&second_record, &item, "tester")
        .unwrap();
    link_hub(store, bank, &rev2, item_a);
    (distilled, rev2, hub_subject, second_record)
}

/// Test-only combined trait object so one suite exercises both contracts.
trait EraseBackend: Store + ComplianceStore {}

/// The erasure derivation closure reaches every revision row of a unit: both
/// revisions quote `item_a` through their own cross-reference edges, so both
/// are purged and unreadable, and the record is retained.
fn suite_erase_purges_a_revised_unit(store: &dyn EraseBackend) {
    let bank = unique("erase-revised");
    let subject = unique("subject");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject), "tester")
        .unwrap();
    let (distilled, rev2, hub_subject, second_record) =
        seed_revised_distilled_unit(store, &bank, &item_a);

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.derived_items_purged, 2, "both revision rows purge");
    assert!(
        store.get_snapshot(&bank, &distilled).is_err(),
        "the prior revision's quote is unreadable"
    );
    assert!(
        store.get_snapshot(&bank, &rev2).is_err(),
        "the current revision's quote is unreadable"
    );
    assert_record_retained(store, &bank, &distilled, &item_a, &second_record);
    assert!(
        !store.subject_dek_destroyed(&hub_subject).unwrap(),
        "the hub's own subject DEK is untouched"
    );
}

#[test]
fn sqlite_erase_purges_a_revised_distilled_unit() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_purges_a_revised_unit(&store);
}

#[test]
fn postgres_erase_purges_a_revised_distilled_unit() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    suite_erase_purges_a_revised_unit(&store);
}

impl EraseBackend for SqliteStore {}
impl EraseBackend for PostgresStore {}

/// A distilled unit whose record carries an evidence trail, so the retention
/// rule is checked against the trail and not only against the claim set.
fn seed_unit_with_evidence(store: &dyn Store, bank: &str, item_a: &str) -> (String, String) {
    let hub = unique("hub");
    let hub_subject = unique("hub-subject");
    let distilled = format!("hub-distill:{bank}:{hub}");
    let mut item = memory_item(bank, &distilled, &hub_subject);
    item.source_ref = Some("hub_distilled".into());
    item.snapshot = Some(serde_json::json!({
        "unit_id": distilled,
        "hub_id": hub,
        "neighbor_ids": [item_a],
        "neighbor_count": 1,
    }));
    let mut record = DistilledUnitRecord::new(
        bank,
        &distilled,
        &hub,
        [item_a.to_owned()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    // Ids and a stamp read from the item rows: exactly what the write path
    // records, and the whole reason the record needs no shredding of its own.
    let source = store.get_memory_item(bank, item_a).unwrap().unwrap();
    record.record_evidence(evidence_from_items([&source]), "2026-06-01T00:00:00Z");
    store
        .commit_distilled_unit(&item, &record, "tester")
        .unwrap();
    link_hub(store, bank, &distilled, item_a);
    (distilled, hub_subject)
}

/// The evidence trail follows exactly the retention rule of the item row it
/// belongs to: the purge clears the row's content and keeps its metadata, so the
/// id-only trail is retained with it, and the record carries nothing that the
/// purge removed.
///
/// A trail that stored a gist or any source text would make this assertion fail,
/// which is the point: the record must not become a second, readable copy of
/// what the purge erased.
fn suite_erase_retains_only_the_ids_of_the_evidence_trail(store: &dyn EraseBackend) {
    let bank = unique("erase-evidence");
    let subject = unique("subject");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject), "tester")
        .unwrap();
    let (distilled, hub_subject) = seed_unit_with_evidence(store, &bank, &item_a);

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.derived_items_purged, 1, "the derived unit purges");
    assert!(
        store.get_snapshot(&bank, &distilled).is_err(),
        "nothing readable of the unit survives: its content is gone"
    );
    assert!(
        store.get_gist(&bank, &distilled).is_err(),
        "and so is its prose half"
    );
    assert!(
        store.get_memory_item(&bank, &distilled).is_err(),
        "and the item itself is unreadable, so no half of it is"
    );

    // The record survives with the row, exactly as the claim set already did.
    let retained = store
        .get_distilled_unit(&bank, &distilled)
        .unwrap()
        .expect("the record is retained with the item row's metadata");
    assert_eq!(retained.proof_count(), 1, "one source in the trail");

    // And nothing in the trail is text. The stored trail is checked two ways:
    // its key set is exactly ids and stamps, and every string it holds is one of
    // the values the write path put there. A gist or a quote would break the
    // first; a substituted value would break the second.
    let trail = serde_json::to_value(retained.evidence_history()).expect("the trail serializes");
    assert_eq!(trail_key_set(&trail), TRAIL_KEYS, "ids and stamps only");
    for value in evidence_strings(&retained) {
        assert!(
            value == item_a || value.parse::<i64>().is_ok() || value == "2026-06-01T00:00:00Z",
            "the trail holds an id or a stamp and nothing else: {value:?}"
        );
    }
    assert!(
        !store.subject_dek_destroyed(&hub_subject).unwrap(),
        "the hub's own subject DEK is untouched"
    );
}

/// Every key name anywhere in a serialized evidence trail, sorted, so the
/// comparison is on the key set rather than on a document's layout.
fn trail_key_set(value: &serde_json::Value) -> Vec<String> {
    let mut keys = Vec::new();
    collect_keys(value, &mut keys);
    keys.sort();
    keys.dedup();
    keys
}

/// The whole key set a trail is allowed to have: the revision, the reconciliation
/// time, and per entry the source id and its stamp.
const TRAIL_KEYS: [&str; 5] = ["as_of", "entries", "reconciled_at", "revision", "source_id"];

fn collect_keys(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                out.push(key.clone());
                collect_keys(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_keys(item, out);
            }
        }
        _ => {}
    }
}

/// Every string the evidence trail holds, so a test can state what each one is
/// allowed to be rather than asserting on a rendered blob.
fn evidence_strings(record: &DistilledUnitRecord) -> Vec<String> {
    let mut out = Vec::new();
    for list in record.evidence_history() {
        out.push(list.reconciled_at.clone());
        for entry in &list.entries {
            out.push(entry.source_id.clone());
            out.extend(entry.as_of.clone());
        }
    }
    out
}

#[test]
fn sqlite_erase_retains_only_the_ids_of_the_evidence_trail() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_retains_only_the_ids_of_the_evidence_trail(&store);
}

#[test]
fn postgres_erase_retains_only_the_ids_of_the_evidence_trail() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    suite_erase_retains_only_the_ids_of_the_evidence_trail(&store);
}

/// Deleting the item row deletes the record with it, trail included. The record
/// never outlives the row it describes, in either direction of that rule.
fn suite_delete_removes_the_record_and_its_trail(store: &dyn EraseBackend) {
    let bank = unique("delete-evidence");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, "subject"), "tester")
        .unwrap();
    let (distilled, _) = seed_unit_with_evidence(store, &bank, &item_a);
    assert!(
        store
            .get_distilled_unit(&bank, &distilled)
            .unwrap()
            .is_some_and(|r| r.proof_count() == 1),
        "the trail is stored"
    );
    assert!(store.delete_item(&bank, &distilled).unwrap());
    assert!(
        store
            .get_distilled_unit(&bank, &distilled)
            .unwrap()
            .is_none(),
        "the record and its trail go with the row"
    );
}

#[test]
fn sqlite_delete_removes_the_record_and_its_trail() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_delete_removes_the_record_and_its_trail(&store);
}

#[test]
fn postgres_delete_removes_the_record_and_its_trail() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    suite_delete_removes_the_record_and_its_trail(&store);
}
