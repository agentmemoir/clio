// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres restore's count-mismatch
//! message: which tables it names, and how it words a count of rows.
//!
//! ## Owns
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
//! Test-only. Never compiled into production builds. These two tests are pure:
//! they build a `DumpMeta` and a count list by hand and call the message
//! builder, so unlike their siblings they take no Postgres test lock and need
//! no database.

use super::{count_mismatch_message, derived_difference_message, result_for};
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
    let message = count_mismatch_message(&meta, &observed_without_items).expect("a mismatch");
    assert!(message.contains("items"), "{message}");
    assert!(message.contains("not counted"), "{message}");

    let observed_matching = vec![("items".to_owned(), 3u64)];
    assert_eq!(count_mismatch_message(&meta, &observed_matching), None);
}

#[test]
fn a_count_mismatch_of_one_row_names_the_row_in_the_singular() {
    // The counts phrase goes through the one shared helper, and a table that
    // differed by a single row is the case where the noun was wrong: "archive 1
    // rows" is the same defect #54 was filed for, on a restore line.
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
    let counted_message = count_mismatch_message(&meta, &counted).expect("a mismatch");

    assert!(
        counted_message.contains("archive 1 row, restored 2"),
        "one row is one row: {counted_message}"
    );
    assert!(
        !counted_message.contains("archive 1 rows"),
        "the plural must not come back: {counted_message}"
    );

    let uncounted =
        count_mismatch_message(&meta, &[("triples".to_owned(), 1u64)]).expect("a mismatch");
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

/// A difference confined to a derived table is not the count mismatch: the
/// user-data message is silent, the derived message names the table and both
/// counts, and the comparison produces the derived result rather than the
/// mismatch result.
#[test]
fn a_derived_table_difference_is_not_a_count_mismatch() {
    let meta = two_table_meta();
    let observed = vec![
        ("items".to_owned(), 3u64),
        ("index_pending".to_owned(), 0u64),
    ];

    assert_eq!(count_mismatch_message(&meta, &observed), None);
    let derived = derived_difference_message(&meta, &observed).expect("a derived difference");
    assert!(
        derived.contains("every user-data row was loaded"),
        "{derived}"
    );
    assert!(derived.contains("index_pending"), "{derived}");
    assert!(derived.contains("archive 2 rows, restored 0"), "{derived}");
    assert_eq!(
        result_for(None, Some(&derived), None),
        RESULT_DERIVED_DIFFERENT
    );
}

/// A difference in a user-data table is still the count mismatch, worded
/// exactly as before, and it still wins over a derived difference beside it.
#[test]
fn a_user_data_difference_is_still_the_count_mismatch() {
    let meta = two_table_meta();
    let observed = vec![
        ("items".to_owned(), 2u64),
        ("index_pending".to_owned(), 0u64),
    ];

    let mismatch = count_mismatch_message(&meta, &observed).expect("a mismatch");
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
    assert_eq!(
        result_for(Some(&mismatch), Some("derived"), None),
        RESULT_COUNT_MISMATCH
    );
    assert_eq!(result_for(None, None, None), RESULT_RESTORED);
    assert_eq!(result_for(None, None, Some("key")), RESULT_PARTIAL);
}
