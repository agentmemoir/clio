// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the item-timestamp backfill tests: a stored
//! epoch-millis value is rewritten into the canonical RFC 3339 form in every
//! item time column, a distilled unit takes its source item's creation time
//! whatever order the rows are visited in, a `NULL` window stays `NULL`, and a
//! second run changes nothing.
//!
//! ## Owns
//! - The rewrite rule's effect on every one of the seven item time columns
//! - The precedence rule between a distilled unit and its hub, and the pass's
//!   idempotency
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

/// A second, later epoch-millis value: 2023-11-14T22:15:00Z.
const HUB_MILLIS: &str = "1700000100000";

/// The canonical form of [`HUB_MILLIS`].
const HUB_MILLIS_RFC3339: &str = "2023-11-14T22:15:00Z";

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

/// Insert one item row whose window columns are set, which is where the nullable
/// item time columns live.
fn insert_window_item(store: &SqliteStore, id: &str) {
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO items
               (id, bank_id, kind, category, epistemic_kind, valid_from, tx_from, created_at,
                updated_at, valid_until, tx_until, discarded_at)
             VALUES (?1, 'bank', 'semantic', 'schema', 'fact', ?2, ?2, ?2, ?2, ?3, NULL, ?4)",
            rusqlite::params![id, MILLIS_RFC3339, MILLIS, MILLIS],
        )
        .expect("insert window item");
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

/// Read one item's seven time columns back.
fn stamps(store: &SqliteStore, id: &str) -> [Option<String>; 7] {
    store
        .conn
        .lock()
        .expect("store lock")
        .query_row(
            "SELECT created_at, valid_from, tx_from, updated_at, valid_until, tx_until, discarded_at
             FROM items WHERE id = ?1",
            [id],
            |r| {
                Ok([
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ])
            },
        )
        .expect("read stamps")
}

/// The four `NOT NULL` columns, as plain strings.
fn required(store: &SqliteStore, id: &str) -> [String; 4] {
    let all = stamps(store, id);
    [0, 1, 2, 3].map(|i| all[i].clone().expect("not-null column"))
}

#[test]
fn a_stored_millis_stamp_is_rewritten_into_the_canonical_form() {
    let store = store();
    insert_item(&store, "plain", MILLIS, MILLIS);

    let rewritten = backfill_sqlite(&store.conn.lock().expect("store lock")).expect("backfill");

    assert_eq!(rewritten, 1);
    assert_eq!(
        required(&store, "plain"),
        [MILLIS_RFC3339; 4],
        "every not-null item time column moved into the canonical form"
    );
}

#[test]
fn a_distilled_unit_takes_its_source_items_creation_time() {
    let store = store();
    insert_item(&store, "hub", "2026-10-12T08:30:01Z", MILLIS);
    insert_item(&store, "unit", MILLIS, MILLIS);
    insert_distilled_unit(&store, "unit", "hub");

    backfill_sqlite(&store.conn.lock().expect("store lock")).expect("backfill");

    let unit = required(&store, "unit");
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
fn a_unit_whose_hub_still_holds_millis_inherits_the_converted_time() {
    let store = store();
    // The unit is inserted first on purpose: the pass must not depend on which
    // row it happens to read first, and the hub's own value is still millis when
    // the unit is decided. The hub's millis names a different second than the
    // unit's, so inheriting and falling back cannot look alike.
    insert_item(&store, "unit", MILLIS, MILLIS);
    insert_item(&store, "hub", HUB_MILLIS, MILLIS);
    insert_distilled_unit(&store, "unit", "hub");

    backfill_sqlite(&store.conn.lock().expect("store lock")).expect("backfill");

    assert_eq!(
        required(&store, "unit")[0],
        HUB_MILLIS_RFC3339,
        "the hub's millis creation time is converted before it is inherited"
    );
    assert_eq!(required(&store, "hub")[0], HUB_MILLIS_RFC3339);
}

#[test]
fn a_millis_window_column_is_rewritten_and_a_null_window_stays_null() {
    let store = store();
    insert_window_item(&store, "closed");

    backfill_sqlite(&store.conn.lock().expect("store lock")).expect("backfill");

    let closed = stamps(&store, "closed");
    assert_eq!(closed[4].as_deref(), Some(MILLIS_RFC3339), "valid_until");
    assert_eq!(closed[5], None, "a NULL tx_until is never written as ''");
    assert_eq!(closed[6].as_deref(), Some(MILLIS_RFC3339), "discarded_at");
}

#[test]
fn a_second_run_changes_nothing() {
    let store = store();
    insert_item(&store, "hub", MILLIS, MILLIS);
    insert_item(&store, "unit", MILLIS, MILLIS);
    insert_distilled_unit(&store, "unit", "hub");
    insert_window_item(&store, "closed");
    backfill_sqlite(&store.conn.lock().expect("store lock")).expect("first backfill");
    let after_first = (
        stamps(&store, "hub"),
        stamps(&store, "unit"),
        stamps(&store, "closed"),
    );

    let rewritten = backfill_sqlite(&store.conn.lock().expect("store lock")).expect("second");

    assert_eq!(rewritten, 0, "a canonical database has nothing to rewrite");
    assert_eq!(
        (
            stamps(&store, "hub"),
            stamps(&store, "unit"),
            stamps(&store, "closed")
        ),
        after_first
    );
}

#[test]
fn a_missing_source_falls_back_to_the_rows_own_value() {
    let row = StoredStamps {
        id: "unit".into(),
        bank_id: "bank".into(),
        created_at: MILLIS.into(),
        valid_from: MILLIS.into(),
        tx_from: MILLIS.into(),
        updated_at: MILLIS.into(),
        valid_until: None,
        tx_until: None,
        discarded_at: None,
        hub_id: Some("gone".into()),
    };

    let fixed = fixed_stamps(&row, None);

    assert_eq!(fixed.created_at, MILLIS_RFC3339);
    assert!(fixed.changed);
}

#[test]
fn another_banks_hub_is_never_used() {
    let store = store();
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO items
               (id, bank_id, kind, category, epistemic_kind, valid_from, tx_from, created_at, updated_at)
             VALUES ('hub', 'other', 'semantic', 'schema', 'fact', ?1, ?1, '2020-01-01T00:00:00Z', ?1)",
            rusqlite::params![MILLIS_RFC3339],
        )
        .expect("insert foreign hub");
    insert_item(&store, "unit", MILLIS, MILLIS);
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO distilled_units (bank_id, item_id, hub_id, marker, distilled_at)
             VALUES ('bank', 'unit', 'hub', 'hub_distilled', ?1)",
            rusqlite::params![MILLIS_RFC3339],
        )
        .expect("insert distilled unit");

    backfill_sqlite(&store.conn.lock().expect("store lock")).expect("backfill");

    assert_eq!(
        required(&store, "unit")[0],
        MILLIS_RFC3339,
        "the lookup is scoped by bank, so a foreign row with the same id is not the hub"
    );
}
