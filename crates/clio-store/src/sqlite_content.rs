// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the content digest: a fingerprint of a SQLite
//! store's logical content that survives page compaction.
//!
//! ## Owns
//! - `sqlite_content_digest`: every user table's rows hashed in primary-key
//!   order
//! - The row canonicalisation (type tag, length, value bytes) that makes the
//!   digest independent of how SQLite laid the file out
//!
//! ## Does not own
//! - Taking the snapshot (see `sqlite_snapshot`)
//! - The archive or its manifest (see `clio-backup`)
//!
//! ## Boundary
//!
//! Changes to how a store's content is fingerprinted should normally require
//! changes here. Changes to how a snapshot is taken should not.
//!
//! Keep this module focused on the content digest.

use crate::migrate::IntoAmError;
use crate::sqlite_snapshot::{hex, select_rows_from, user_tables};
use clio_types::AmError;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Digest the logical content of a SQLite store over primary-key-ordered rows.
///
/// A `VACUUM INTO` copy is compacted, so its bytes differ from the source even
/// when the content is identical. This digest is the comparison that survives
/// compaction: it hashes every user table's rows in primary-key order, so a
/// changed value, a missing row, or a reordered page all change the digest,
/// while a re-laid-out page does not.
///
/// Each row contributes a type tag, a length, and the value bytes for every
/// column, so a `NULL` and an empty string cannot collide.
///
/// # Errors
/// The file is missing, unreadable, or holds a table the digest cannot order.
pub fn sqlite_content_digest(path: &Path) -> Result<String, AmError> {
    // The digest can run on the live store, so it holds the store shared for
    // its lifetime; a concurrent restore's exclusive hold makes the acquire
    // wait a bounded time and then refuse. A path that is not there at all is
    // refused by the open below, and leaves no lock file behind.
    let _store_lock = if path.is_file() {
        Some(crate::store_lock::SharedHold::acquire(path)?)
    } else {
        None
    };
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .am("open the store for a content digest")?;
    // The digest can run on the live store, so the open-handle count must
    // observe its connection too, the same way the snapshot registers its own.
    let _handle = crate::open_handles::register(path);
    let mut hasher = Sha256::new();
    for table in user_tables(&conn)? {
        hasher.update(table.as_bytes());
        hasher.update([0u8]);
        let shape = table_shape(&conn, &table)?;
        let sql = select_rows_from(&table, &order_clause(&shape));
        let mut statement = conn.prepare(&sql).am("prepare the content digest query")?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rows = statement.query([]).am("read rows for the content digest")?;
        while let Some(row) = rows.next().am("step the content digest query")? {
            for (index, name) in names.iter().enumerate() {
                hasher.update(name.as_bytes());
                let value = row.get_ref(index).am("read a digest column")?;
                hash_value(&mut hasher, &value);
            }
            hasher.update([0xFF]);
        }
    }
    Ok(hex(&hasher.finalize()))
}

/// A table's columns, split into the ones the digest orders by and the rest.
///
/// `WITHOUT ROWID` tables and single-column keys both report correctly through
/// `PRAGMA table_info`, which gives every column's name and its position in the
/// declared primary key (0 when the column is not part of the key).
#[derive(Debug, Default, PartialEq, Eq)]
struct TableShape {
    /// Primary-key columns in key order; empty when the table declares no key.
    key: Vec<String>,
    /// Every column in table order.
    all: Vec<String>,
}

impl TableShape {
    /// The columns the digest orders rows by.
    ///
    /// A declared key is used when there is one. Otherwise every column is,
    /// which is what makes the row order total: two rows that tie on the key
    /// would otherwise come back in whatever order the file happens to store
    /// them, and the same content could hash two ways. Ordering by every column
    /// cannot tie at all except between rows that are equal in every column,
    /// and those contribute identical bytes, so the digest stays stable either
    /// way.
    fn order_by(&self) -> &[String] {
        if self.key.is_empty() {
            &self.all
        } else {
            &self.key
        }
    }
}

/// The declared primary-key columns and all columns of `table`.
fn table_shape(conn: &Connection, table: &str) -> Result<TableShape, AmError> {
    let mut statement = conn
        .prepare(&format!(
            "PRAGMA table_info(\"{}\")",
            table.replace('"', "\"\"")
        ))
        .am("read table columns")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })
        .am("read table columns")?;
    let mut shape = TableShape::default();
    let mut keyed: Vec<(i64, String)> = Vec::new();
    for row in rows {
        let (name, position) = row.am("read a table column")?;
        if position > 0 {
            keyed.push((position, name.clone()));
        }
        shape.all.push(name);
    }
    keyed.sort_by_key(|(position, _)| *position);
    shape.key = keyed.into_iter().map(|(_, name)| name).collect();
    Ok(shape)
}

/// The `ORDER BY` clause for a content digest: the primary key, or every
/// column when the table declares none, so the row order is total either way.
fn order_clause(shape: &TableShape) -> String {
    shape
        .order_by()
        .iter()
        .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Hash one column value with a type tag and a length, so distinct values
/// cannot be spelled the same way.
fn hash_value(hasher: &mut Sha256, value: &ValueRef<'_>) {
    match value {
        ValueRef::Null => hasher.update([0u8]),
        ValueRef::Integer(n) => {
            hasher.update([1u8]);
            hasher.update(n.to_le_bytes());
        }
        ValueRef::Real(f) => {
            hasher.update([2u8]);
            hasher.update(f.to_bits().to_le_bytes());
        }
        ValueRef::Text(bytes) => {
            hasher.update([3u8]);
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        ValueRef::Blob(bytes) => {
            hasher.update([4u8]);
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
    }
}

#[cfg(test)]
#[path = "sqlite_content_order_tests.rs"]
mod order_tests;
