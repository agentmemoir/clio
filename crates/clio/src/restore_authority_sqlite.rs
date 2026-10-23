// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading subject references out of the SQLite
//! payload and key store an archive carries, and for turning them into
//! membership facts.
//!
//! ## Owns
//! - `classify_sqlite_payload`, `classify_key_store`, and `live_key_hashes`
//! - The read-only SQLite column reader `sqlite_column`, `sqlite_column_where`,
//!   `column_present`, and `column_exists`
//!
//! ## Does not own
//! - The archive's classification as a whole (`restore_authority` owns that)
//! - The Postgres dump-table reads (`restore_authority_dump` owns those)
//!
//! ## Boundary
//!
//! Changes to how a SQLite payload or key store names its subjects should
//! normally require changes here. Changes to the authority rules should not.
//!
//! Keep this module focused on the SQLite side of one archive.

use super::{
    ClassifyRefusal, ITEMS_TABLE, KEY_COLUMN, KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN,
    SUBJECT_KEYS_TABLE, TOMBSTONE_HASH_COLUMN, TOMBSTONE_TABLE, facts_from_hashes,
};
use clio_backup::ExtractedMember;
use clio_compliance::{MemberFacts, tombstone_salt};
use clio_store::sha256_hex;
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeSet;
use std::path::Path;

/// Classify a SQLite payload: its tombstones, subject keys, and item subjects.
///
/// An item row keeps its subject link after the subject's key was destroyed,
/// so the item's subject id is a presence the archive carries.
pub(super) fn classify_sqlite_payload(
    payload: &Path,
    live: &BTreeSet<String>,
    keys_present: bool,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let mut hashes = BTreeSet::new();
    for hash in sqlite_column(payload, TOMBSTONE_TABLE, TOMBSTONE_HASH_COLUMN)? {
        if !hash.is_empty() {
            hashes.insert(hash);
        }
    }
    let salt = tombstone_salt();
    for id in sqlite_column(payload, SUBJECT_KEYS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    for id in sqlite_column(payload, ITEMS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    Ok(facts_from_hashes(hashes, live, keys_present))
}

/// The subjects whose key slot in the archive's key store still carries a
/// usable key, as deployment-wide subject hashes.
///
/// A slot whose key column is unreadable is treated as live, so an archive that
/// cannot be shown to have destroyed the key fails closed. A key member that
/// does not carry the slot table this build reads cannot be classified at all,
/// so it refuses rather than yielding an empty live set.
pub(super) fn live_key_hashes(
    key: Option<&ExtractedMember>,
) -> Result<BTreeSet<String>, ClassifyRefusal> {
    let Some(key) = key else {
        return Ok(BTreeSet::new());
    };
    let path = key.file.path();
    if !column_present(path, KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN)? {
        return Err(ClassifyRefusal::KeyStoreUnreadable);
    }
    let ids = if column_present(path, KEY_SLOTS_TABLE, KEY_COLUMN)? {
        sqlite_column_where(
            path,
            KEY_SLOTS_TABLE,
            SUBJECT_ID_COLUMN,
            "key_b64 IS NOT NULL",
        )?
    } else {
        sqlite_column(path, KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN)?
    };
    let salt = tombstone_salt();
    Ok(ids.into_iter().map(|id| sha256_hex(&salt, &id)).collect())
}

/// Classify the archive's key member: one fact per key slot id.
pub(super) fn classify_key_store(
    key: Option<&ExtractedMember>,
    live: &BTreeSet<String>,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let Some(key) = key else {
        return Ok(vec![MemberFacts::None]);
    };
    let salt = tombstone_salt();
    let mut hashes = BTreeSet::new();
    for id in sqlite_column(key.file.path(), KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    // The key member is present by definition here, so its slot facts are
    // classified against the usable-key set.
    Ok(facts_from_hashes(hashes, live, true))
}

/// Every value of `column` in `table`, read from the SQLite database at `path`.
///
/// A missing table or column yields no values: the store simply holds no such
/// record. A NULL value is skipped: it names no subject, which is what the
/// `COPY` text reader does with its `\N` marker. The connection is opened
/// read-write with `query_only` set, the pattern that lets a store recover a
/// hot journal while a statement still cannot write.
fn sqlite_column(path: &Path, table: &str, column: &str) -> Result<Vec<String>, ClassifyRefusal> {
    sqlite_column_where(path, table, column, "")
}

/// Every value of `column` in the rows of `table` matching `predicate`.
///
/// `predicate` is a constant SQL expression owned by this module; it never
/// carries caller input.
fn sqlite_column_where(
    path: &Path,
    table: &str,
    column: &str,
    predicate: &str,
) -> Result<Vec<String>, ClassifyRefusal> {
    let connection =
        rusqlite::Connection::open(path).map_err(|error| sqlite_error(path, table, &error))?;
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(|error| sqlite_error(path, table, &error))?;
    if !column_exists(&connection, table, column)
        .map_err(|error| sqlite_error(path, table, &error))?
    {
        return Ok(Vec::new());
    }
    let sql = if predicate.is_empty() {
        format!("SELECT {column} FROM {table}")
    } else {
        format!("SELECT {column} FROM {table} WHERE {predicate}")
    };
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| sqlite_error(path, table, &error))?;
    let rows = statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| sqlite_error(path, table, &error))?;
    let mut values = Vec::new();
    for row in rows {
        if let Some(value) = row.map_err(|error| sqlite_error(path, table, &error))? {
            values.push(value);
        }
    }
    Ok(values)
}

/// Whether `table` holds a column named `column` in the database at `path`.
fn column_present(path: &Path, table: &str, column: &str) -> Result<bool, ClassifyRefusal> {
    let connection =
        rusqlite::Connection::open(path).map_err(|error| sqlite_error(path, table, &error))?;
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(|error| sqlite_error(path, table, &error))?;
    column_exists(&connection, table, column).map_err(|error| sqlite_error(path, table, &error))
}

/// Whether `table` holds a column named `column`.
fn column_exists(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        rusqlite::params![table, column],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// The refusal for a SQLite read failure, naming the table it was reading.
fn sqlite_error(path: &Path, table: &str, error: &rusqlite::Error) -> ClassifyRefusal {
    ClassifyRefusal::Io(AmError::new(
        ErrorCode::Internal,
        format!("reading table {table:?} from {}: {error}", path.display()),
    ))
}
