// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite distilled-unit write paths owned by
//! `sqlite_distilled_unit` that move an existing record: the claim/stale update
//! with its snapshot re-projection and every "leave the record alone" early
//! return, the missing-record refusal, and the operations-removal cascade over
//! one bank's records.
//!
//! ## Owns
//! - The update, refusal, and cascade paths of that module
//!
//! ## Does not own
//! - The revision and read/list paths (`sqlite_distilled_unit_tests`)
//! - The backend-agnostic cascade contract (`distilled_unit_discard_cascade_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what an update
//! writes when the current revision is unreadable, or to what the cascade
//! leaves behind, should normally update tests here.

use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{DistilledUnitRecord, STALE_SOURCE_REMOVED};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::sqlite::SqliteStore;
use crate::sqlite_distilled_unit::sqlite_distilled_unit_tests::{exec, seed_source};
use crate::store::Store;
use clio_types::ErrorCode;
use rusqlite::ToSql;
use serde_json::json;

#[test]
fn an_update_persists_the_claim_set_and_reprojects_the_snapshot() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-update");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
    let mut unit = record(&bank, &item_id, &hub, &[a.clone(), b.clone()]);
    store.commit_distilled_unit(&item, &unit, "tester").unwrap();

    unit.source_ids.retain(|id| id != &b);
    unit.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    unit.stale_at = Some("2026-06-02T00:00:00Z".to_owned());
    store.update_distilled_unit(&unit, "tester").unwrap();

    let stored = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(stored.source_ids, vec![a.clone()]);
    assert_eq!(stored.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(stored.stale_at.as_deref(), Some("2026-06-02T00:00:00Z"));

    let row = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    let snapshot = row.snapshot.expect("the snapshot is re-projected");
    assert_eq!(snapshot["neighbor_ids"], json!([a]));
    assert_eq!(snapshot["neighbor_count"], json!(1));
    assert_eq!(snapshot["stale_reason"], json!(STALE_SOURCE_REMOVED));
    assert_eq!(snapshot["hub_id"], json!(hub));
}

#[test]
fn an_update_skips_a_current_revision_closed_outside_reconciliation() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-update-closed");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let mut unit = record(&bank, &item_id, &hub, &[]);
    store.commit_distilled_unit(&item, &unit, "tester").unwrap();
    exec(
        &store,
        "UPDATE items SET tx_until = ?1 WHERE id = ?2 AND bank_id = ?3",
        &[
            &"2026-06-05T00:00:00Z" as &dyn ToSql,
            &item_id as &dyn ToSql,
            &bank as &dyn ToSql,
        ],
    );

    unit.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    unit.stale_at = Some("2026-06-06T00:00:00Z".to_owned());
    store
        .update_distilled_unit(&unit, "tester")
        .expect("a historical row is not an error");

    let stored = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert!(
        stored.stale_reason.is_none(),
        "a closed revision is never rewritten"
    );
}

#[test]
fn an_update_leaves_the_record_alone_when_the_body_is_unreadable() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-update-dead");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let mut unit = record(&bank, &item_id, &hub, &[]);
    store.commit_distilled_unit(&item, &unit, "tester").unwrap();
    unit.stale_reason = Some(STALE_SOURCE_REMOVED.to_owned());
    unit.stale_at = Some("2026-06-06T00:00:00Z".to_owned());

    let update = "UPDATE items SET content_ciphertext = ?1 WHERE id = ?2 AND bank_id = ?3";
    for body in [None, Some(""), Some("not-a-ciphertext-envelope")] {
        exec(
            &store,
            update,
            &[
                &body as &dyn ToSql,
                &item_id as &dyn ToSql,
                &bank as &dyn ToSql,
            ],
        );
        store
            .update_distilled_unit(&unit, "tester")
            .unwrap_or_else(|e| panic!("dead unit body {body:?} must not fail: {e}"));
        let stored = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
        assert!(
            stored.stale_reason.is_none(),
            "body {body:?} is erasure evidence"
        );
    }

    exec(
        &store,
        update,
        &[
            &None::<String> as &dyn ToSql,
            &item_id as &dyn ToSql,
            &bank as &dyn ToSql,
        ],
    );
    exec(
        &store,
        "UPDATE items SET subject_id = NULL WHERE id = ?1 AND bank_id = ?2",
        &[&item_id as &dyn ToSql, &bank as &dyn ToSql],
    );
    store.update_distilled_unit(&unit, "tester").unwrap();
    let stored = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert!(
        stored.stale_reason.is_none(),
        "no subject means a dead unit"
    );
}

#[test]
fn an_update_without_a_stored_record_reports_not_found() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-update-missing");
    let subject = unique("subject");
    let hub = unique("hub");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[]);
    let unit = record(&bank, &item_id, &hub, &[]);
    store.commit_distilled_unit(&item, &unit, "tester").unwrap();
    exec(
        &store,
        "DELETE FROM distilled_units WHERE item_id = ?1 AND bank_id = ?2",
        &[&item_id as &dyn ToSql, &bank as &dyn ToSql],
    );

    let err = store.update_distilled_unit(&unit, "tester").unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains(&item_id), "{}", err.message);
}

#[test]
fn a_discarded_source_shortens_the_claim_set_and_marks_the_unit_stale() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-cascade");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    for id in [&hub, &a, &b] {
        seed_source(&store, &bank, &subject, id);
    }
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
    store
        .commit_distilled_unit(
            &item,
            &record(&bank, &item_id, &hub, &[a.clone(), b.clone()]),
            "tester",
        )
        .unwrap();

    // A second unit in the same bank claims neither id being discarded: the
    // cascade must walk past it and leave its record untouched.
    let other_hub = unique("hub-other");
    let other_id = format!("hub-distill:{bank}:{other_hub}");
    let other = hub_item(&bank, &other_id, &subject, &other_hub, &[]);
    store
        .commit_distilled_unit(&other, &record(&bank, &other_id, &other_hub, &[]), "tester")
        .unwrap();

    store
        .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();

    let after = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        after.source_ids,
        vec![b.clone()],
        "the removed id is dropped"
    );
    assert_eq!(after.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(after.stale_at.as_deref(), Some("2026-06-02T00:00:00Z"));
    assert_eq!(after.revision, 1, "the cascade writes no revision");

    let untouched = store.get_distilled_unit(&bank, &other_id).unwrap().unwrap();
    assert!(
        untouched.stale_reason.is_none(),
        "a unit that claims no removed id keeps its state"
    );

    let trail = store.audit_trail(&bank, &item_id).unwrap();
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "hub_distill_source_removed"),
        "the cascade is audited"
    );
}

#[test]
fn a_discarded_hub_stays_a_claim_and_a_dead_unit_record_is_left_alone() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-cascade-hub");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let dead_hub = unique("hub-dead");
    let dead_source = unique("source-dead");
    for id in [&hub, &a, &dead_hub, &dead_source] {
        seed_source(&store, &bank, &subject, id);
    }

    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    store
        .commit_distilled_unit(
            &item,
            &record(&bank, &item_id, &hub, std::slice::from_ref(&a)),
            "tester",
        )
        .unwrap();

    let dead_id = format!("hub-distill:{bank}:{dead_hub}");
    let dead_item = hub_item(&bank, &dead_id, &subject, &dead_hub, &[&dead_source]);
    store
        .commit_distilled_unit(
            &dead_item,
            &record(
                &bank,
                &dead_id,
                &dead_hub,
                std::slice::from_ref(&dead_source),
            ),
            "tester",
        )
        .unwrap();
    // The dead unit's current body no longer opens: erasure evidence.
    exec(
        &store,
        "UPDATE items SET content_ciphertext = NULL WHERE id = ?1 AND bank_id = ?2",
        &[&dead_id as &dyn ToSql, &bank as &dyn ToSql],
    );

    store
        .discard_item(&bank, &hub, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();
    let after = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        after.source_ids,
        vec![a.clone()],
        "the hub column is the unit's identity, not a source to drop"
    );
    assert_eq!(after.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));

    store
        .discard_item(
            &bank,
            &dead_source,
            "withdrawn",
            "2026-06-03T00:00:00Z",
            "tester",
        )
        .unwrap();
    let dead = store.get_distilled_unit(&bank, &dead_id).unwrap().unwrap();
    assert_eq!(
        dead.source_ids,
        vec![dead_source.clone()],
        "an unreadable unit keeps its record"
    );
    assert!(
        dead.stale_reason.is_none(),
        "a dead unit's record is erasure evidence"
    );
}

/// The record the cascade wrote is the record a later read sees: the stored
/// claim set and the item snapshot agree.
#[test]
fn the_cascade_leaves_record_and_snapshot_in_agreement() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("b-cascade-agree");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    for id in [&hub, &a, &b] {
        seed_source(&store, &bank, &subject, id);
    }
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&a, &b]);
    store
        .commit_distilled_unit(
            &item,
            &record(&bank, &item_id, &hub, &[a.clone(), b.clone()]),
            "tester",
        )
        .unwrap();
    store
        .discard_item(&bank, &b, "withdrawn", "2026-06-04T00:00:00Z", "tester")
        .unwrap();

    let stored: DistilledUnitRecord = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    let row = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    let snapshot = row.snapshot.expect("snapshot");
    let projected: Vec<String> =
        serde_json::from_value(snapshot["neighbor_ids"].clone()).expect("ids");
    assert_eq!(projected, stored.source_ids);
    assert_eq!(snapshot["neighbor_count"], json!(stored.source_ids.len()));
}
