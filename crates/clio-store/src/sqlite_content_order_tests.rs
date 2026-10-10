// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the content digest's row order: proving the
//! digest of a table with no declared primary key does not depend on the order
//! the rows happen to sit in the file.
//!
//! ## Owns
//! - The keyless-table ordering cases, where the primary key is absent and the
//!   order therefore comes from every column
//!
//! ## Does not own
//! - The snapshot copy, its refusal cases, or the store's integrity check (see
//!   `sqlite_snapshot_tests`)
//! - The key-store member (see `snapshot_keys_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on how the digest orders rows.

use super::{TableShape, order_clause, sqlite_content_digest};
use rusqlite::Connection;
use std::path::PathBuf;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-content-order-{tag}"
        )))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A database holding one keyless table `plain(a, b)` with the given rows, in
/// the order they are written.
fn keyless_store(path: &PathBuf, rows: &[(&str, &str)]) {
    let conn = Connection::open(path).expect("open");
    conn.execute_batch("CREATE TABLE plain (a, b);")
        .expect("create a keyless table");
    for (a, b) in rows {
        conn.execute("INSERT INTO plain VALUES (?1, ?2)", (*a, *b))
            .expect("insert a row");
    }
}

/// The digest of a keyless table, read back after the rows are deleted and
/// reinserted in a different order.
///
/// Both databases hold the same content, so the digest has to match: a digest
/// that followed the file's row order would be a fingerprint of the file's
/// layout, which is exactly what a `VACUUM INTO` copy changes.
fn digest_across_insert_orders(rows: &[(&str, &str)], reordered: &[(&str, &str)]) -> String {
    let scratch = Scratch::new("keyless");
    let first = scratch.join("first.db");
    let second = scratch.join("second.db");
    keyless_store(&first, rows);
    keyless_store(&second, reordered);
    let one = sqlite_content_digest(&first).expect("digest the first store");
    let other = sqlite_content_digest(&second).expect("digest the reordered store");
    assert_eq!(
        one, other,
        "the digest followed the row order, not the content"
    );
    one
}

#[test]
fn a_keyless_table_digests_the_same_whatever_order_the_rows_were_inserted() {
    digest_across_insert_orders(
        &[("a", "one"), ("b", "two"), ("c", "three")],
        &[("c", "three"), ("a", "one"), ("b", "two")],
    );
}

#[test]
fn a_keyless_table_that_ties_on_its_first_column_still_digests_by_content() {
    // Every row shares the same first column, so ordering by that column alone
    // leaves the remaining rows in whatever order the file stores them. The
    // digest has to come out the same anyway, which takes every column into the
    // order rather than the first one.
    digest_across_insert_orders(
        &[("same", "one"), ("same", "two"), ("same", "three")],
        &[("same", "three"), ("same", "one"), ("same", "two")],
    );
}

#[test]
fn a_keyless_table_still_digests_a_changed_value() {
    let scratch = Scratch::new("changed");
    let path = scratch.join("plain.db");
    keyless_store(&path, &[("a", "one"), ("a", "two")]);
    let before = sqlite_content_digest(&path).expect("digest");
    let conn = Connection::open(&path).expect("reopen");
    conn.execute("UPDATE plain SET b = 'edited' WHERE b = 'one'", [])
        .expect("edit a value");

    assert_ne!(
        sqlite_content_digest(&path).expect("digest after the edit"),
        before,
        "a changed value must change the digest"
    );
}

#[test]
fn a_declared_key_is_preferred_over_every_column() {
    let shape = TableShape {
        key: vec!["k".to_owned()],
        all: vec!["k".to_owned(), "v".to_owned()],
    };

    assert_eq!(order_clause(&shape), "\"k\"");
}

#[test]
fn a_keyless_table_orders_by_every_column_in_table_order() {
    let shape = TableShape {
        key: Vec::new(),
        all: vec!["a".to_owned(), "b".to_owned()],
    };

    assert_eq!(order_clause(&shape), "\"a\", \"b\"");
}

#[test]
fn a_column_name_needing_quotes_is_quoted_in_the_order_clause() {
    let shape = TableShape {
        key: Vec::new(),
        all: vec!["we\"ird".to_owned()],
    };

    assert_eq!(order_clause(&shape), "\"we\"\"ird\"");
}
