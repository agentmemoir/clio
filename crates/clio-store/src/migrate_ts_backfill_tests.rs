// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the item-timestamp backfill tests: a stored
//! epoch-millis value is rewritten into the canonical RFC 3339 form, a
//! distilled unit takes its source item's creation time, and a second run
//! changes nothing.
//!
//! ## Owns
//! - The rewrite rule's effect on every one of the four item time columns
//! - The pass's idempotency
//!
//! ## Does not own
//! - The canonical form's own construction (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{StoredStamps, backfill_sqlite, fixed_stamps};
use crate::LocalDevKms;
use crate::SqliteStore;
use std::sync::Arc;

/// Epoch millis no other fixture in this suite produces: 2023-11-14T22:13:20Z.
const MILLIS: &str = "1700000000000";

/// The canonical form of [`MILLIS`].
const MILLIS_RFC3339: &str = "2023-11-14T22:13:20Z";

/// A store whose schema this build bootstrapped, with the backfill already run
/// on an empty table.
fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("open in-memory store")
}

/// Insert one item row with exactly the columns the backfill reads.
fn insert_item(store: &SqliteStore, id: &str, created_at: &str, updated_at: &str) {
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO items
               (id, bank_id, kind, category, epistemic_kind, valid_from, tx_from, created_at, updated_at)
             VALUES (?1, 'bank', 'semantic', 'schema', 'fact', ?2, ?2, ?3, ?4)",
            rusqlite::params![id, updated_at, created_at, updated_at],
        )
        .expect("insert item");
}

/// Record `unit` as distilled from `hub`, which is what names the source item.
fn insert_distilled_unit(store: &SqliteStore, unit: &str, hub: &str) {
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO distilled_units (bank_id, item_id, hub_id, marker, distilled_at)
             VALUES ('bank', ?1, ?2, 'hub_distilled', ?3)",
            rusqlite::params![unit, hub, MILLIS],
        )
        .expect("insert distilled unit");
}

/// Read one item's four time columns back.
fn stamps(store: &SqliteStore, id: &str) -> Vec<String> {
    store
        .conn
        .lock()
        .expect("store lock")
        .query_row(
            "SELECT created_at, valid_from, tx_from, updated_at FROM items WHERE id = ?1",
            [id],
            |r| Ok(vec![r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]),
        )
        .expect("read stamps")
}

#[test]
fn a_stored_millis_stamp_is_rewritten_into_the_canonical_form() {
    let store = store();
    insert_item(&store, "plain", MILLIS, MILLIS);

    let rewritten = backfill_sqlite(&mut store.conn.lock().expect("store lock")).expect("backfill");

    assert_eq!(rewritten, 1);
    assert_eq!(
        stamps(&store, "plain"),
        [MILLIS_RFC3339; 4],
        "every item time column moved into the canonical form"
    );
}

#[test]
fn a_distilled_unit_takes_its_source_items_creation_time() {
    let store = store();
    insert_item(&store, "hub", "2026-10-12T08:30:01Z", MILLIS);
    insert_item(&store, "unit", MILLIS, MILLIS);
    insert_distilled_unit(&store, "unit", "hub");

    backfill_sqlite(&mut store.conn.lock().expect("store lock")).expect("backfill");

    let unit = stamps(&store, "unit");
    assert_eq!(
        unit[0], "2026-10-12T08:30:01Z",
        "created_at is the moment the distilled unit summarizes, not the moment it was written"
    );
    assert_eq!(
        [unit[1].as_str(), unit[2].as_str(), unit[3].as_str()],
        [MILLIS_RFC3339; 3],
        "the other three columns stay the distillation moment"
    );
}

#[test]
fn a_second_run_changes_nothing() {
    let store = store();
    insert_item(&store, "hub", "2026-10-12T08:30:01Z", MILLIS);
    insert_item(&store, "unit", MILLIS, MILLIS);
    insert_distilled_unit(&store, "unit", "hub");
    backfill_sqlite(&mut store.conn.lock().expect("store lock")).expect("first backfill");
    let after_first = (stamps(&store, "hub"), stamps(&store, "unit"));

    let rewritten = backfill_sqlite(&mut store.conn.lock().expect("store lock")).expect("second");

    assert_eq!(rewritten, 0, "a canonical database has nothing to rewrite");
    assert_eq!((stamps(&store, "hub"), stamps(&store, "unit")), after_first);
}

#[test]
fn a_missing_source_falls_back_to_the_rows_own_value() {
    let row = StoredStamps {
        id: "unit".into(),
        created_at: MILLIS.into(),
        valid_from: MILLIS.into(),
        tx_from: MILLIS.into(),
        updated_at: MILLIS.into(),
        hub_id: Some("gone".into()),
    };

    let fixed = fixed_stamps(&row, None);

    assert_eq!(fixed.created_at, MILLIS_RFC3339);
    assert!(fixed.changed);
}
