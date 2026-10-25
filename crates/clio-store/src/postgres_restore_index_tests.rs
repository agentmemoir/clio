// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the one fact a restore report carries about the derived
//! index: whether its truncate really emptied a table the store rebuilds. It
//! builds reports through the assembler the restore itself uses, so the fact is
//! pinned as what the transaction reported and never as something a count, a
//! backend, or the archive's shape implies.
//!
//! ## Owns
//! - The report's `derived_index_emptied` value against the truncate's answer
//! - That a count difference leaves that fact alone
//!
//! ## Does not own
//! - The count rule itself (see `postgres_restore_rows_tests`)
//! - The truncate statement and the transaction that runs it
//!   (see `postgres_restore_tests`, which needs a live target)
//!
//! ## Boundary
//!
//! Tests here break when the report stops carrying the truncation the restore
//! observed; they never change that behaviour. Never compiled into production
//! builds.
//!
//! Keep this module focused on the truncation fact the rebuild notice reads.

use super::{PostgresRestoreRequest, RestoreTable, restore_report};
use std::path::PathBuf;

/// A request whose one table is `items`, which is user data: no count here can
/// make the report claim a derived table was emptied.
fn request() -> PostgresRestoreRequest {
    PostgresRestoreRequest {
        url: "postgres://target".to_owned(),
        schema: "clio".to_owned(),
        tables: vec![RestoreTable {
            table: "items".to_owned(),
            columns: vec!["id".to_owned(), "bank_id".to_owned()],
            member_path: PathBuf::from("member_000.txt"),
            rows: 2,
        }],
    }
}

/// The counts the restore observed for that table.
fn observed(count: u64) -> Vec<(String, u64)> {
    vec![("items".to_owned(), count)]
}

/// The report repeats what the truncate reported: the same counts carry the
/// fact either way, so nothing in the comparison can manufacture or erase it.
#[test]
fn the_report_repeats_the_truncation_the_transaction_reported() {
    let request = request();

    let emptied = restore_report(&request, observed(2), Vec::new(), true);
    assert!(
        emptied.derived_index_emptied,
        "the truncate said it emptied the index: {emptied:?}"
    );

    let untouched = restore_report(&request, observed(2), Vec::new(), false);
    assert!(
        !untouched.derived_index_emptied,
        "the truncate said it emptied nothing: {untouched:?}"
    );
    assert_eq!(
        emptied.counts_ok, untouched.counts_ok,
        "the fact is independent of the count rule"
    );
}

/// A load that lost a row is still a run whose index is empty: the fact stays
/// true beside the mismatch, so the rebuild an operator needs is not withdrawn
/// because the counts went wrong.
#[test]
fn a_count_difference_leaves_the_truncation_fact_alone() {
    let request = request();

    let report = restore_report(&request, observed(1), Vec::new(), true);
    assert!(
        !report.counts_ok,
        "a lost row is not a clean count: {report:?}"
    );
    assert!(
        report.derived_index_emptied,
        "the index is empty whatever the counts say: {report:?}"
    );
}
