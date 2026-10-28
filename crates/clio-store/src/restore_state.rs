// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for measuring one SQLite store file without
//! writing it: whether it exists, how many bytes it holds, and the row count of
//! every user table, which is what a restore's blast radius is built from.
//!
//! ## Owns
//! - `SqliteFileState`, the read-only measurement of one file
//! - The bank-id list the blast radius names
//! - The empty-target rule (every counted table holds no rows)
//!
//! ## Does not own
//! - Taking the snapshot or placing a restore (see `sqlite_snapshot` and
//!   `restore_swap`)
//! - The table enumeration and counting SQL (see `sqlite_snapshot`, which the
//!   snapshot path also uses, so the two cannot disagree)
//!
//! ## Boundary
//!
//! Changes to how a target is measured should normally require changes here.
//! Changes to how a snapshot is taken or placed should not.
//!
//! Keep this module focused on the read-only measurement of one store file.

use crate::migrate::{IntoAmError, map_db_err};
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// What one SQLite store file holds, measured read-only.
///
/// A missing file is a valid measurement, not an error: a restore into a fresh
/// path is one of the cases the command supports, and `exists: false` is how
/// the report says so instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteFileState {
    /// The file that was measured.
    pub path: PathBuf,
    /// Whether the file exists.
    pub exists: bool,
    /// Bytes on disk; zero when the file does not exist.
    pub bytes: u64,
    /// Every user table with its row count, ordered by table name. Empty when
    /// the file does not exist.
    pub table_counts: Vec<(String, u64)>,
    /// The distinct bank ids of the `items` table, ordered by bank id. Empty
    /// when the file does not exist or holds no `items` table.
    pub banks: Vec<String>,
}

impl SqliteFileState {
    /// The row count of one table, or `None` when the file has no such table.
    #[must_use]
    pub fn count_of(&self, table: &str) -> Option<u64> {
        self.table_counts
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, count)| *count)
    }

    /// How many user tables were counted.
    #[must_use]
    pub fn table_count(&self) -> usize {
        self.table_counts.len()
    }

    /// Whether every counted table holds no rows.
    ///
    /// This is what a restore's "nothing to preserve" skip keys on, so it
    /// counts *every* user table rather than the two the reports lead with. A
    /// store can hold beliefs, persona preferences, tasks, subject keys, or sync
    /// cursors while `items` and `triples` are both empty, and a restore
    /// replaces the whole file: deciding "nothing to lose" from those two tables
    /// alone would destroy the rest of it with no snapshot to go back to.
    ///
    /// The cost is that a store opened once is not empty, because bootstrapping
    /// writes settings rows, so a genuinely fresh store takes a snapshot too.
    /// That is one small archive, and it is the safe direction to be wrong in.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table_counts.iter().all(|(_, count)| *count == 0)
    }
}

/// Measure `path` read-only: existence, size, every user table's row count,
/// and the `items` table's distinct bank ids.
///
/// The file is opened `SQLITE_OPEN_READ_ONLY` and nothing is written: a
/// measurement that migrated, bootstrapped, or checkpointed would change the
/// store it is about to describe. A file that exists but cannot be read as a
/// database is an error rather than an empty measurement, because reporting
/// "no rows" for a file nobody could read is the one answer that would let a
/// restore overwrite it on a false premise.
///
/// # Errors
/// The path exists but is not a regular file, or cannot be opened and counted
/// as a SQLite database.
pub fn sqlite_file_state(path: &Path) -> Result<SqliteFileState, AmError> {
    file_state(path, true)
}

/// Measure a SQLite file that is not the live store, without taking the
/// store's cross-process hold.
///
/// The staged payload a restore measures before placing it is a candidate
/// file, not a store: it is never the file a concurrent restore would
/// replace, and taking a hold on it would leave a permanent lock file beside
/// a temporary file, which is exactly what a dry run must not do. Anything
/// that opens the live store keeps using [`sqlite_file_state`].
///
/// # Errors
/// The file is missing, unreadable, or holds a table the measurement cannot
/// read.
pub fn sqlite_file_state_unheld(path: &Path) -> Result<SqliteFileState, AmError> {
    file_state(path, false)
}

/// The measurement both variants share; the hold, when taken, lives to the
/// end of the measurement.
fn file_state(path: &Path, hold: bool) -> Result<SqliteFileState, AmError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} exists but is not a regular file, so it cannot be measured as a store",
                    path.display()
                ),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SqliteFileState {
                path: path.to_path_buf(),
                exists: false,
                bytes: 0,
                table_counts: Vec::new(),
                banks: Vec::new(),
            });
        }
        Err(error) => return Err(map_db_err("reading the store file", error)),
    };
    // The store's derived tables are vector virtual tables, so the backend
    // extension has to be registered before a count reads them; this is the
    // same registration every store open performs through the `vector_backend`
    // seam, not a second copy of it.
    crate::vector_backend::ensure_registered()?;
    // This measurement is a real store open, so it holds the store shared for
    // its lifetime; during the swap this hold is granted nested inside the
    // restore's own exclusive hold. The unheld variant skips this for files
    // that are not the live store.
    let _store_lock = if hold {
        Some(crate::store_lock::SharedHold::acquire(path)?)
    } else {
        None
    };
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .am("open the store read-only to measure it")?;
    // The measurement's connection is a real open store handle, so it is
    // registered for the connection's lifetime: the swap's open-handle count
    // then observes every connection this measurement opens, and a
    // connection that leaks instead of dropping refuses the swap.
    let _handle = crate::open_handles::register(path);
    let table_counts = crate::sqlite_snapshot::count_user_tables(&conn)?;
    let banks = read_bank_ids(&conn, table_counts.iter().any(|(name, _)| name == "items"))?;
    drop(conn);
    Ok(SqliteFileState {
        path: path.to_path_buf(),
        exists: true,
        bytes: metadata.len(),
        table_counts,
        banks,
    })
}

/// The distinct bank ids of the `items` table, ordered by bank id.
///
/// A bank id is operator-chosen text that the blast radius names, so it is
/// read here, beside the counts, from the same read-only connection. Empty
/// when the store has no `items` table. An `items` table without a `bank_id`
/// column is an error rather than an empty list: a file whose memory table
/// does not have the store's shape is not a file this build replaces on the
/// strength of a half measurement.
fn read_bank_ids(conn: &Connection, has_items: bool) -> Result<Vec<String>, AmError> {
    if !has_items {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare("SELECT DISTINCT bank_id FROM items ORDER BY bank_id")
        .am("list the store's banks")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .am("read the store's banks")?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.am("read a bank id")?);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "restore_state_tests.rs"]
mod tests;
