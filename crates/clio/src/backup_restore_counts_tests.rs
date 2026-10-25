// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres restore's count comparison, over
//! hand-built facts: which arm each difference lands on, how a count of rows is
//! worded, and which result the observation produces.
//!
//! ## Owns
//! - The direction rule's four arms: a deficit anywhere, a user-data surplus, a
//!   derived surplus, and equal counts
//! - The mismatch message's two arms: a table the target counted differently,
//!   and a table the target never counted at all
//! - The row count's noun, which goes through the one shared helper so a
//!   one-row table reads "1 row" on both arms
//!
//! ## Does not own
//! - The Postgres restore's refusals and its rollback (see
//!   `backup_restore_postgres_refusal_tests`)
//! - The counts the SQLite restore prints (see `backup_restore_report`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. These tests are pure: they
//! build a `DumpMeta` and a count list by hand and call the comparison, so
//! unlike their siblings they take no Postgres test lock and need no database.

use super::comparison;
use crate::backup_restore_flow::{
    RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT, RESULT_PARTIAL, RESULT_RESTORED,
};
use clio_backup::dump_meta::{DUMP_FORMAT_VERSION, DUMP_KIND, DumpColumn, DumpMeta, DumpTable};
use clio_store::{PG_CONSISTENCY, PG_METHOD};

#[test]
fn a_count_mismatch_names_a_table_the_target_never_counted() {
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        tables: vec![DumpTable {
            table: "items".to_owned(),
            member: "dump/tables/001-items.txt".to_owned(),
            bytes: 4,
            sha256: "0".repeat(64),
            rows: 3,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "bigint".to_owned(),
            }],
        }],
    };

    // The target's measurement has no `items` row at all: the mismatch says
    // the table was not counted, not that it held zero rows.
    let observed_without_items = vec![("triples".to_owned(), 1u64)];
    let message = comparison(&meta, &observed_without_items, None)
        .1
        .expect("a mismatch");
    assert!(message.contains("items"), "{message}");
    assert!(message.contains("not counted"), "{message}");

    let observed_matching = vec![("items".to_owned(), 3u64)];
    assert_eq!(comparison(&meta, &observed_matching, None).1, None);
}

#[test]
fn a_count_mismatch_of_one_row_names_the_row_in_the_singular() {
    // The counts phrase goes through the one shared helper, and a table that
    // differed by a single row is the case where the noun was wrong: "archive 1
    // rows" is the same defect, on a restore line.
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        tables: vec![DumpTable {
            table: "items".to_owned(),
            member: "dump/tables/001-items.txt".to_owned(),
            bytes: 4,
            sha256: "0".repeat(64),
            rows: 1,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "bigint".to_owned(),
            }],
        }],
    };

    let counted = vec![("items".to_owned(), 2u64)];
    let counted_message = comparison(&meta, &counted, None).1.expect("a mismatch");

    assert!(
        counted_message.contains("archive 1 row, restored 2"),
        "one row is one row: {counted_message}"
    );
    assert!(
        !counted_message.contains("archive 1 rows"),
        "the plural must not come back: {counted_message}"
    );

    let uncounted = comparison(&meta, &[("triples".to_owned(), 1u64)], None)
        .1
        .expect("a mismatch");
    assert!(
        uncounted.contains("archive 1 row, restored: not counted"),
        "and the not-counted arm says the same: {uncounted}"
    );
}

/// One archive table with a declared row count.
fn table(name: &str, member: &str, rows: u64) -> DumpTable {
    DumpTable {
        table: name.to_owned(),
        member: member.to_owned(),
        bytes: 4,
        sha256: "0".repeat(64),
        rows,
        columns: vec![DumpColumn {
            name: "id".to_owned(),
            ordinal: 1,
            type_name: "bigint".to_owned(),
        }],
    }
}

/// An archive holding the two tables one derived row and three user-data rows
/// live in.
fn two_table_meta() -> DumpMeta {
    DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        tables: vec![
            table("items", "dump/tables/001-items.txt", 3),
            table("index_pending", "dump/tables/002-index_pending.txt", 2),
        ],
    }
}

/// A count list holding one observation per archived table.
fn observed(items: u64, derived: u64) -> Vec<(String, u64)> {
    vec![
        ("items".to_owned(), items),
        ("index_pending".to_owned(), derived),
    ]
}

/// Equal counts on both tables are the clean restore: no message from either
/// arm and no failed step behind the result.
#[test]
fn equal_counts_are_the_clean_restore() {
    let meta = two_table_meta();
    let (result, mismatch, derived) = comparison(&meta, &observed(3, 2), None);
    assert_eq!(result, RESULT_RESTORED);
    assert_eq!(mismatch, None);
    assert_eq!(derived, None);
}

/// A derived table the restore over-filled is the one benign difference: the
/// user-data message is silent, the derived message names the table and both
/// counts, and the result is the derived one rather than the mismatch.
#[test]
fn a_derived_surplus_is_the_benign_derived_difference() {
    let meta = two_table_meta();
    let (result, mismatch, derived) = comparison(&meta, &observed(3, 5), None);

    assert_eq!(mismatch, None);
    let derived = derived.expect("a derived difference");
    assert_eq!(
        derived,
        "every user-data row was loaded; a table the store rebuilds holds a surplus that \
         neither the reindex a restore prints nor a retry of the restore clears, and the index \
         pipeline reconciles it on its next pass: table \"index_pending\": archive 2 rows, \
         restored 5",
        "the operator reads what a retry does and does not clear"
    );
    assert_eq!(result, RESULT_DERIVED_DIFFERENT);
}

/// A derived table the restore under-filled is rows the archive held and the
/// restore did not load: the hard count mismatch, for a rebuildable table as
/// much as for a user-data one.
#[test]
fn a_derived_deficit_is_a_hard_count_mismatch() {
    let meta = two_table_meta();
    let (result, mismatch, derived) = comparison(&meta, &observed(3, 1), None);

    let mismatch = mismatch.expect("the deficit is a mismatch");
    assert!(
        mismatch.starts_with(
            "the restored table counts do not match the archive's: table \"index_pending\""
        ),
        "{mismatch}"
    );
    assert!(
        mismatch.contains("archive 2 rows, restored 1"),
        "{mismatch}"
    );
    assert_eq!(derived, None, "a deficit is never the benign difference");
    assert_eq!(result, RESULT_COUNT_MISMATCH);
}

/// A user-data table holding more rows than the archive recorded is invented
/// data, not a rebuildable difference: the hard count mismatch both ways.
#[test]
fn a_user_data_surplus_is_a_hard_count_mismatch() {
    let meta = two_table_meta();
    let (result, mismatch, derived) = comparison(&meta, &observed(4, 2), None);

    let mismatch = mismatch.expect("a surplus is a mismatch");
    assert!(mismatch.contains("table \"items\""), "{mismatch}");
    assert!(
        mismatch.contains("archive 3 rows, restored 4"),
        "{mismatch}"
    );
    assert_eq!(derived, None);
    assert_eq!(result, RESULT_COUNT_MISMATCH);
}

/// A user-data table holding fewer rows than the archive recorded is lost data,
/// worded exactly as it always was, and it wins over a derived surplus beside
/// it: a benign difference never masks a lost row.
#[test]
fn a_user_data_deficit_is_a_hard_count_mismatch() {
    let meta = two_table_meta();
    let (result, mismatch, derived) = comparison(&meta, &observed(2, 5), None);

    let mismatch = mismatch.expect("a deficit is a mismatch");
    assert!(
        mismatch
            .starts_with("the restored table counts do not match the archive's: table \"items\""),
        "{mismatch}"
    );
    assert!(
        mismatch.contains("archive 3 rows, restored 2"),
        "{mismatch}"
    );
    assert!(
        !mismatch.contains("index_pending"),
        "the derived table is not the user-data message: {mismatch}"
    );
    assert!(derived.is_some(), "the derived surplus is still worded");
    assert_eq!(result, RESULT_COUNT_MISMATCH);
}

/// The key store outranks a benign derived difference and not a lost row: the
/// result is the partial restore only when no count is a hard mismatch.
#[test]
fn a_failed_key_store_is_partial_until_a_count_is_mismatched() {
    let meta = two_table_meta();
    assert_eq!(
        comparison(&meta, &observed(3, 2), Some("key")).0,
        RESULT_PARTIAL
    );
    assert_eq!(
        comparison(&meta, &observed(3, 5), Some("key")).0,
        RESULT_PARTIAL,
        "the derived surplus does not outrank the key store"
    );
    assert_eq!(
        comparison(&meta, &observed(3, 1), Some("key")).0,
        RESULT_COUNT_MISMATCH,
        "the lost row does outrank the key store"
    );
}
