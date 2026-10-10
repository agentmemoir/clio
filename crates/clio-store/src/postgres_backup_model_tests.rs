// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pure Postgres backup-half contract: identifier
//! quoting, dump member numbering, and the table-set comparison.
//!
//! ## Owns
//! - The assertions for the quoted-identifier rules, the member names a dump
//!   assigns, and the set comparison that refuses a mismatch
//!
//! ## Does not own
//! - The contract itself (see `postgres_backup_model`)
//! - The dump, discovery, and restore halves that call into it
//!
//! ## Boundary
//!
//! Test-only, and never compiled into production builds.

use super::*;

#[test]
fn quote_ident_doubles_embedded_quotes_and_nothing_else() {
    assert_eq!(quote_ident("items"), "\"items\"");
    assert_eq!(quote_ident("odd\"name"), "\"odd\"\"name\"");
    assert_eq!(quote_ident(""), "\"\"");
    assert_eq!(quote_ident("a b.c"), "\"a b.c\"");
}

#[test]
fn member_name_numbers_in_the_discovered_order() {
    assert_eq!(
        member_name(1, "assoc_edges").unwrap(),
        "dump/tables/001_assoc_edges.txt"
    );
    assert_eq!(
        member_name(28, "triples").unwrap(),
        "dump/tables/028_triples.txt"
    );
    assert_eq!(member_name(999, "x").unwrap(), "dump/tables/999_x.txt");
}

#[test]
fn member_name_refuses_an_ordinal_outside_the_numbering() {
    assert_eq!(
        member_name(0, "items").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        member_name(1000, "items").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn member_name_refuses_an_empty_or_unembeddable_table_name() {
    assert_eq!(
        member_name(1, "").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        member_name(1, "a/b").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        member_name(1, "a\\b").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn member_name_refuses_a_name_over_the_container_limit() {
    let long = "t".repeat(MAX_MEMBER_NAME_BYTES - "dump/tables/001_.txt".len() + 1);
    assert_eq!(
        member_name(1, &long).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    // Exactly at the limit is still fine.
    let exact = "t".repeat(MAX_MEMBER_NAME_BYTES - "dump/tables/001_.txt".len());
    assert!(member_name(1, &exact).is_ok());
}

#[test]
fn member_name_refuses_a_non_printable_table_name() {
    assert_eq!(
        member_name(1, "a\u{7f}b").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

fn lift<'a>(sets: &[(&'a str, &'a [&'a str])]) -> Vec<(String, Vec<String>)> {
    sets.iter()
        .map(|(name, columns)| {
            (
                (*name).to_owned(),
                columns.iter().map(|c| (*c).to_owned()).collect(),
            )
        })
        .collect()
}

fn sets<'a>(
    target: &[(&'a str, &'a [&'a str])],
    archive: &[(&'a str, &'a [&'a str])],
) -> (Vec<(String, Vec<String>)>, Vec<(String, Vec<String>)>) {
    (lift(target), lift(archive))
}

#[test]
fn compare_table_sets_accepts_matching_and_extra_target_columns() {
    let (target, archive) = sets(
        &[("items", &["id", "bank_id", "added_later"])],
        &[("items", &["id", "bank_id"])],
    );
    assert!(compare_table_sets(&target, &archive).is_ok());
}

#[test]
fn compare_table_sets_refuses_a_table_only_the_archive_has() {
    let (target, archive) = sets(
        &[("items", &["id"])],
        &[("items", &["id"]), ("triples", &["id"])],
    );
    let error = compare_table_sets(&target, &archive).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("the archive carries \"triples\""));
    // Both sets are named, so the drift is visible without opening anything.
    assert!(error.message.contains("triples"));
    assert!(error.message.contains("items"));
}

#[test]
fn compare_table_sets_refuses_a_table_only_the_target_has() {
    let (target, archive) = sets(
        &[("items", &["id"]), ("failures", &["id"])],
        &[("items", &["id"])],
    );
    let error = compare_table_sets(&target, &archive).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("the target has \"failures\""));
}

#[test]
fn compare_table_sets_refuses_a_missing_target_column() {
    let (target, archive) = sets(&[("items", &["id"])], &[("items", &["id", "bank_id"])]);
    let error = compare_table_sets(&target, &archive).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("has no column \"bank_id\""));
}

#[test]
fn compare_table_sets_accepts_two_empty_sets() {
    assert!(compare_table_sets(&[], &[]).is_ok());
}
