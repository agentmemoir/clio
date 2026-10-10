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

use super::count_mismatch_message;
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
