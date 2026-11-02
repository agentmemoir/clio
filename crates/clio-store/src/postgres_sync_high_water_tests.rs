// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests Postgres parity for the per-device high-water mark: the
//! first read is empty, a higher stamp raises the mark, a lower stamp leaves it
//! alone, and each bank, device, and entity kind keeps its own mark.
//!
//! ## Owns
//! - Postgres parity coverage for `SyncStore::device_high_water_get` and
//!   `SyncStore::device_high_water_set`
//!
//! ## Does not own
//! - The SQLite path (`sync_store_tests`)
//! - The table's creation on an already-bootstrapped database
//!   (`sync_schema_converge_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres high-water SQL changes; they never change
//! it.
//!
//! Keep this module focused on the Postgres high-water mark.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

/// The mark starts empty, only ever rises, and is scoped to one bank, device,
/// and entity kind.
#[test]
fn pg_high_water_never_moves_backwards() {
    let _guard = lock_pg_test();
    let bank = unique("hw-bank");
    let other_bank = unique("hw-bank-other");
    let s = open_pg();

    assert!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get")
            .is_none()
    );
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-02T00:00:02Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-02T00:00:02Z".into())
    );
    // A lower stamp leaves the stored mark alone.
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-02T00:00:01Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-02T00:00:02Z".into()),
        "a lower stamp never lowers the mark"
    );
    // A higher stamp raises it.
    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-02T00:00:03Z")
        .expect("set");
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get"),
        Some("2026-01-02T00:00:03Z".into())
    );
    // Each device and bank keeps its own mark.
    assert!(
        s.device_high_water_get(&bank, "dev-other", "item")
            .expect("get")
            .is_none(),
        "each device keeps its own mark"
    );
    assert!(
        s.device_high_water_get(&other_bank, "dev-pg", "item")
            .expect("get")
            .is_none(),
        "each bank keeps its own mark"
    );
}

/// One device stamps different encodings per entity kind: ISO-8601 seconds for
/// one kind, a bare date or epoch millis for another. A per-device mark would
/// compare those shapes as text and dead-letter a legitimate row, so the mark is
/// per kind. The same device and stamp under two kinds therefore stay separate.
#[test]
fn pg_high_water_is_independent_per_entity_kind() {
    let _guard = lock_pg_test();
    let bank = unique("hw-kind-bank");
    let s = open_pg();

    s.device_high_water_set(&bank, "dev-pg", "item", "2026-01-02T00:00:00Z")
        .expect("set the item mark");
    // A second kind under the same device and bank: a bare date, which sorts
    // before every ISO stamp as text.
    s.device_high_water_set(&bank, "dev-pg", "triple", "2026-01-01")
        .expect("set the triple mark");

    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "item")
            .expect("get item"),
        Some("2026-01-02T00:00:00Z".into()),
        "the item mark keeps its own encoding"
    );
    assert_eq!(
        s.device_high_water_get(&bank, "dev-pg", "triple")
            .expect("get triple"),
        Some("2026-01-01".into()),
        "the triple mark is not lowered by the later item stamp"
    );
    assert!(
        s.device_high_water_get(&bank, "dev-pg", "belief")
            .expect("get belief")
            .is_none(),
        "an unwritten kind has no mark"
    );
}
