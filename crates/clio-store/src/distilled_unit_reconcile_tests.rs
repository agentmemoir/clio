// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the distilled-unit reconciliation state
//! contract tests: the v2 record fields and validation, revision id
//! derivation, the revision write (prior closed on both axes, retained,
//! linked, record updated, record/snapshot agreement), the claim/stale update
//! with its snapshot re-projection, and the bank-scoped record list.
//!
//! ## Owns
//! - SQLite revision/update/list scenarios and the v2 record validation tests
//!
//! ## Does not own
//! - The discard cascade scenarios (`discard_cascade_tests`)
//! - Postgres scenarios (`postgres_distilled_unit_tests`)
//! - Engine-level reconcile decisions (`clio-write` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_revision::snapshot_projection;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{
    DISTILLED_UNIT_RECORD_VERSION, DistilledUnitRecord, STALE_REVISION_REJECTED,
    STALE_SOURCE_REMOVED,
};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::collections::BTreeSet;

/// A well-formed record with sources, matching what `hub_item` projects.
pub(crate) fn record(
    bank: &str,
    item_id: &str,
    hub: &str,
    sources: &[String],
) -> DistilledUnitRecord {
    DistilledUnitRecord::new(
        bank,
        item_id,
        hub,
        sources.iter().cloned(),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    )
}

/// Raw lineage-column read for one item row (the columns are DB-only).
fn lineage_columns(store: &SqliteStore, item_id: &str) -> (Option<String>, Option<String>) {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT superseded_by_id, corrects_item_id FROM items WHERE id = ?1",
                [item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap()
}

#[test]
fn revision_ids_derive_from_the_record_revision() {
    let unit = "hub-distill:b:h";
    assert_eq!(crate::distilled_unit::revision_item_id(unit, 1), unit);
    assert_eq!(
        crate::distilled_unit::revision_item_id(unit, 2),
        format!("{unit}:v2")
    );
    assert_eq!(
        crate::distilled_unit::revision_item_id(unit, 12),
        format!("{unit}:v12")
    );
    let r = record("b", unit, "h", &["s".to_owned()]);
    assert_eq!(r.current_revision_item_id(), unit);
    assert_eq!(r.prior_revision_item_id(), None);
    let r2 = DistilledUnitRecord {
        revision: 3,
        ..record("b", unit, "h", &[])
    };
    assert_eq!(r2.current_revision_item_id(), format!("{unit}:v3"));
    assert_eq!(r2.prior_revision_item_id(), Some(format!("{unit}:v2")));
}

#[test]
fn the_projection_names_the_unit_the_hub_the_sources_and_the_stale_reason() {
    let r = record("b", "u", "h", &["s1".to_owned(), "s2".to_owned()]);
    assert_eq!(
        snapshot_projection(&r),
        serde_json::json!({"unit_id": "u", "hub_id": "h",
            "neighbor_ids": ["s1", "s2"], "neighbor_count": 2})
    );
    let mut stale = r.clone();
    stale.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    assert_eq!(
        snapshot_projection(&stale)["stale_reason"],
        serde_json::json!(STALE_SOURCE_REMOVED)
    );
}

#[test]
fn the_v2_record_validation_rules_bind_the_stale_state() {
    let mut r = record("b", "u", "h", &["s".to_owned()]);
    r.validate().unwrap_or_else(|e| panic!("valid: {e}"));
    assert_eq!(r.revision, 1);
    assert_eq!(r.record_version, DISTILLED_UNIT_RECORD_VERSION);

    r.stale_reason = Some(STALE_REVISION_REJECTED.to_owned());
    assert!(
        r.validate().is_err(),
        "a stale reason without stale_at is invalid"
    );
    r.stale_at = Some("2026-06-02T00:00:00Z".to_owned());
    r.validate().unwrap_or_else(|e| panic!("valid: {e}"));

    r.stale_reason = None;
    assert!(
        r.validate().is_err(),
        "stale_at without a stale reason is invalid"
    );
    r.stale_at = None;
    r.validate().unwrap_or_else(|e| panic!("valid: {e}"));

    r.stale_reason = Some("made_up_reason".to_owned());
    assert!(
        r.validate().is_err(),
        "an unknown stale reason must be refused"
    );

    r.stale_reason = None;
    r.revision = 0;
    assert!(r.validate().is_err(), "revision must be >= 1");
}

#[test]
fn sqlite_list_distilled_units_is_bank_scoped_and_ordered_by_unit_id() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-list");
    let subject = unique("subject");
    let mut ids = Vec::new();
    for hub in ["h2", "h1"] {
        let item_id = format!("hub-distill:{bank}:{hub}");
        let item = hub_item(&bank, &item_id, &subject, hub, &[]);
        let r = record(&bank, &item_id, hub, &[]);
        store.commit_distilled_unit(&item, &r, "tester").unwrap();
        ids.push(item_id);
    }
    ids.sort();
    let listed = store.list_distilled_units(&bank).unwrap();
    assert_eq!(
        listed.iter().map(|r| r.item_id.clone()).collect::<Vec<_>>(),
        ids,
        "the list is ordered by unit id"
    );
    let other_bank_units = store.list_distilled_units(&unique("other-bank")).unwrap();
    assert!(other_bank_units.is_empty(), "{other_bank_units:?}");
}

#[test]
fn sqlite_revise_closes_the_prior_on_both_axes_and_keeps_it() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-revise");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();

    let b = unique("source-b");
    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&b));
    second_record.revision = 2;
    second_record.distilled_at = "2026-06-02T00:00:00Z".to_owned();
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&b],
    );
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    // The prior revision is closed on both axes, retained, and linked.
    let prior = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(prior.gist, first.gist, "the prior text is retained");
    assert_eq!(prior.valid_until.as_deref(), Some("2026-06-02T00:00:00Z"));
    assert_eq!(prior.tx_until.as_deref(), Some("2026-06-02T00:00:00Z"));
    assert_eq!(lineage_columns(&store, &item_id).0, Some(second.id.clone()));

    // The current revision resolves through the record, not the unit id.
    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read, second_record);
    let current = store
        .get_memory_item(&bank, &read.current_revision_item_id())
        .unwrap()
        .unwrap();
    assert_eq!(current.id, format!("{item_id}:v2"));
    assert_eq!(
        lineage_columns(&store, &current.id).1,
        Some(item_id.clone())
    );
    assert_eq!(current.tx_until, None);

    // Record and snapshot agree after the revision.
    let snapshot = store
        .get_snapshot(&bank, &read.current_revision_item_id())
        .unwrap()
        .unwrap();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    claimed.insert(snapshot["hub_id"].as_str().unwrap().to_owned());
    for id in snapshot["neighbor_ids"].as_array().unwrap() {
        claimed.insert(id.as_str().unwrap().to_owned());
    }
    assert_eq!(read.claim_ids(), claimed);
}

#[test]
fn sqlite_revise_refuses_a_wrong_revision_id_or_a_closed_prior() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-revise-refuse");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();

    // A revision item id that is not the record's next revision id is refused
    // before any write.
    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
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
        "no item may be written when the revision is refused"
    );

    // The honest revision closes the prior...
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

    // ...and a second revision over the closed prior is refused: the record
    // still says revision 2, so the derived prior id is the closed row, and
    // the tx_until guard holds.
    let third_record = second_record.clone();
    let third = hub_item(
        &bank,
        &third_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    let err = store
        .revise_distilled_unit(&third_record, &third, "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn sqlite_update_distilled_unit_drops_the_claim_and_marks_stale() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-update");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let r = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store.commit_distilled_unit(&item, &r, "tester").unwrap();

    let mut updated = r.clone();
    updated.source_ids = Vec::new();
    updated.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    updated.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
    store.update_distilled_unit(&updated, "tester").unwrap();

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(read.stale_at.as_deref(), Some("2026-06-03T00:00:00Z"));
    assert_eq!(read.source_ids, Vec::<String>::new());
    assert_eq!(read.revision, 1, "no revision happened");
    assert_eq!(
        read.distilled_at, r.distilled_at,
        "no distillation happened"
    );

    // The current item's snapshot was re-projected from the same record state.
    let snapshot = store.get_snapshot(&bank, &item_id).unwrap().unwrap();
    assert_eq!(snapshot, snapshot_projection(&read));
    // The gist half is untouched by the claim update.
    let gist = store.get_gist(&bank, &item_id).unwrap().unwrap();
    assert_eq!(gist, item.gist.unwrap());
}

/// `correct` keeps working on a distilled item after a reconcile: the
/// correction targets the current revision item id and routes through the
/// same supersession the correction path always used.
#[test]
fn sqlite_correct_still_works_on_a_distilled_item_after_a_reconcile() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-correct");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();
    // A reconcile revision moves the unit to its second revision row.
    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    second_record.distilled_at = "2026-06-02T00:00:00Z".to_owned();
    let rev2 = second_record.current_revision_item_id();
    let second = hub_item(&bank, &rev2, &subject, &hub, &[&a]);
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    // The operator corrects the unit through its current revision id.
    let ts = "2026-06-03T00:00:00Z".to_owned();
    let mut corrected = hub_item(&bank, &unique("corrected"), &subject, &hub, &[&a]);
    corrected.valid_from = ts.clone();
    corrected.tx_from = ts.clone();
    corrected.created_at = ts.clone();
    corrected.updated_at = ts.clone();
    let outcome = store
        .correct_item(&crate::audit_model::CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: rev2.clone(),
            new_item: corrected.clone(),
            reason: "the summary misstated the hub".to_owned(),
            actor: "tester".to_owned(),
            timestamp: ts,
        })
        .unwrap();
    assert_eq!(outcome.prior_item_id, rev2);
    assert_eq!(outcome.new_item_id, corrected.id);
    assert!(!outcome.already_superseded);
    // The corrected item carries the distilled provenance marker through.
    let read = store
        .get_memory_item(&bank, &outcome.new_item_id)
        .unwrap()
        .unwrap();
    assert_eq!(read.source_ref.as_deref(), Some("hub_distilled"));
}

#[test]
fn sqlite_the_record_columns_are_ids_and_timestamps_only() {
    // The record carries ids and timestamps only: no content, gist, or key
    // column can exist. The reconciliation-state columns (record shape v2)
    // are the same rule: revision number and stale reason/detail codes.
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let columns = store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(distilled_units)")
                .am("distilled unit pragma prepare")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .am("distilled unit pragma query")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("distilled unit pragma rows")
        })
        .unwrap();
    assert_eq!(
        columns,
        [
            "bank_id",
            "item_id",
            "hub_id",
            "source_ids_json",
            "marker",
            "record_version",
            "distilled_at",
            "revision",
            "stale_reason",
            "stale_detail",
            "stale_at",
            "evidence_json",
        ]
    );
}

#[test]
fn sqlite_update_distilled_unit_leaves_an_erased_unit_untouched() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("reconcile-erased");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let r = record(&bank, &item_id, &hub, &[]);
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    // Simulate the erasure purge: content gone, record row left behind.
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE items SET content_ciphertext = NULL WHERE id = ?1",
                [item_id.as_str()],
            )
            .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap();

    let mut updated = r.clone();
    updated.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    updated.stale_at = Some("2026-06-03T00:00:00Z".to_owned());
    store.update_distilled_unit(&updated, "tester").unwrap();
    assert_eq!(
        store.get_distilled_unit(&bank, &item_id).unwrap().unwrap(),
        r,
        "a dead unit's record must stay untouched"
    );
}
