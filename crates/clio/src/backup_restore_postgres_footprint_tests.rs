// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres restore's projected pre-restore snapshot
//! footprint: that a small archive restored over a large store reserves room
//! for the store, and that the fallback cannot under-reserve.
//!
//! ## Owns
//! - `projected_snapshot_bytes`, the room estimate the pre-restore snapshot
//!   makes
//!
//! ## Does not own
//! - `plan_postgres`, which calls it (see `backup_restore_postgres`)
//! - The space check itself (see `clio-store`'s restore pre-flight)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::projected_snapshot_bytes;
use clio_backup::{DumpColumn, DumpMeta, DumpTable};
use clio_store::PostgresTargetState;

/// A target state with the given `table -> rows` counts and no tables.
///
/// The projection reads only the row counts, so no column list is built.
fn state(counts: &[(&str, u64)]) -> PostgresTargetState {
    PostgresTargetState {
        schema: "am_projection".to_owned(),
        tables: Vec::new(),
        table_counts: counts
            .iter()
            .map(|(name, rows)| ((*name).to_owned(), *rows))
            .collect(),
        banks: Vec::new(),
    }
}

/// A dump meta whose `items` member holds `rows` rows in `bytes` bytes.
fn meta(rows: u64, bytes: u64) -> DumpMeta {
    DumpMeta {
        kind: "clio.dump.meta".to_owned(),
        format_version: 1,
        backend: "postgres".to_owned(),
        schema: "am_projection".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: "postgres-copy".to_owned(),
        consistency: "repeatable read".to_owned(),
        tables: vec![DumpTable {
            table: "items".to_owned(),
            member: "dump/tables/001_items.txt".to_owned(),
            bytes,
            sha256: "a".repeat(64),
            rows,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "text".to_owned(),
            }],
        }],
    }
}

#[test]
fn a_small_archive_over_a_large_store_reserves_more_than_the_archive() {
    // The pre-restore snapshot is a fresh dump of the TARGET, so its footprint
    // comes from the target's measured rows scaled by the archive's own
    // bytes-per-row. An archive of 1 row in 100 bytes restored over a 1000-row
    // target reserves 100_000 bytes, not 100.
    let projected = projected_snapshot_bytes(&state(&[("items", 1_000)]), &meta(1, 100));
    assert_eq!(
        projected, 100_000,
        "a small archive over a large store must not under-reserve"
    );
    assert!(
        projected > 100,
        "the reservation is larger than the archive it came from"
    );
}

#[test]
fn an_empty_target_reserves_nothing() {
    assert_eq!(projected_snapshot_bytes(&state(&[]), &meta(1, 100)), 0);
    assert_eq!(
        projected_snapshot_bytes(&state(&[("items", 0), ("triples", 0)]), &meta(1, 100)),
        0
    );
}

#[test]
fn an_archive_with_no_rows_falls_back_to_the_target_row_total() {
    // No ratio can be applied, so the floor is reserved: one byte a row. The
    // check may over-reserve; it must never under-reserve.
    assert_eq!(
        projected_snapshot_bytes(&state(&[("items", 500)]), &meta(0, 0)),
        500
    );
    assert_eq!(
        projected_snapshot_bytes(&state(&[("items", 500)]), &meta(0, 100)),
        500
    );
    // A meta that declares rows but no member bytes has no ratio either.
    assert_eq!(
        projected_snapshot_bytes(&state(&[("items", 500)]), &meta(7, 0)),
        500
    );
}

#[test]
fn the_projection_rounds_up_so_it_never_under_reserves() {
    // 3 rows in 10 bytes is 3.33 bytes a row; the ceiling makes it 4, so the
    // projection is 4_000 rather than 3_333. A floor would under-reserve.
    let projected = projected_snapshot_bytes(&state(&[("items", 1_000)]), &meta(3, 10));
    assert_eq!(projected, 4_000);
    assert!(projected > 1_000 * 10 / 3);
}

#[test]
fn the_projection_sums_rows_across_every_table() {
    let target = state(&[("items", 100), ("triples", 300), ("banks", 600)]);
    assert_eq!(projected_snapshot_bytes(&target, &meta(1, 10)), 10_000);
}

#[test]
fn the_projection_never_returns_less_than_the_target_row_total() {
    // A member byte count of 1 across 10 declared rows would give a sub-byte
    // ratio; the floor keeps the reservation at one byte a row.
    let projected = projected_snapshot_bytes(&state(&[("items", 400)]), &meta(10, 1));
    assert_eq!(projected, 400);
}
