// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for copy-inventory metadata persistence: one
//! row per inventoried copy on both backends, saved and read back as
//! custody and operator-action metadata, with no copy content.
//!
//! ## Owns
//! - `CopyInventoryStore`, the save/load contract, and `CopyInventoryRow`
//! - The per-backend SQL for the insert-or-refuse save, the restart reads,
//!   and the named-open-gap read (both drivers, in this module)
//! - The convergence call sites: the legacy repair itself lives in
//!   `copy_inventory_converge`
//!
//! ## Does not own
//! - The `copy_inventory` slot DDL text (loaded through `migrate`)
//! - The custody classification rules and the copy-result vocabulary:
//!   `custody`, `owner`, `required_action`, `state`, and `exception_reason`
//!   are stored verbatim
//! - Copy staging, copying, deletion, or any storage of copy content
//!
//! ## Boundary
//!
//! Changes to which columns the store writes or reads should normally require
//! changes here. Changes to how a copy is classified or acted on should not
//! require changes here.
//!
//! Keep this module focused on the copy-inventory metadata contract.

use crate::copy_inventory_converge::{converge_copy_inventory_sqlite, converge_pg};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::OptionalExtension;

/// One copy-inventory row, as saved and read back.
///
/// The row carries metadata only: which request the copy belongs to, its
/// custody class, its owner, the required operator action, the current
/// state, an optional exception reason naming an open gap, and the last
/// update time. No copy content, digest, token, or key travels here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyInventoryRow {
    /// The stable copy id; unique across all stored copies.
    pub copy_id: String,
    /// The request the copy is attached to, fixed when the copy is inventoried.
    pub request_id: String,
    /// The custody class of the copy, fixed when the copy is inventoried.
    pub custody: String,
    /// The owner of the copy.
    pub owner: String,
    /// The action an operator or the next run must take for this copy.
    pub required_action: String,
    /// The current copy state, including the final outcome.
    pub state: String,
    /// Why the copy has no request result yet, when it is an open gap.
    pub exception_reason: Option<String>,
    /// The last update time, RFC3339 UTC.
    pub updated_at: String,
}

/// Persistence contract for copy-inventory metadata.
///
/// Implementations MUST check the stored request and custody inside the
/// write transaction, so a reused copy id bound to a different request or
/// custody is refused without mutating anything, even when two writers
/// race. A restart reads rows back verbatim: nothing is auto-deleted or
/// auto-completed.
///
/// The convention is metadata-only: the stored fields carry custody, owner,
/// required-action, state, and exception metadata, and a caller must never
/// route subject content — copy bytes, a digest of subject data, a token, or
/// a key — through this store.
pub trait CopyInventoryStore: Send + Sync {
    /// Save one copy row.
    ///
    /// The first save for a `copy_id` inserts the row. A re-save of the
    /// identical row is a no-op, so a retried write after a restart cannot
    /// double-apply. A re-save with the same request and custody updates the
    /// row; one with a different request or custody is refused and changes
    /// nothing.
    ///
    /// # Errors
    /// Blank required fields, a reused id bound to a different request or
    /// custody, or storage failures.
    fn save(&self, row: &CopyInventoryRow) -> Result<(), AmError>;

    /// The stored row for `copy_id`, if any.
    ///
    /// # Errors
    /// Storage failures.
    fn load(&self, copy_id: &str) -> Result<Option<CopyInventoryRow>, AmError>;

    /// Every stored row, ordered by copy id — the restart read.
    ///
    /// # Errors
    /// Storage failures.
    fn load_all(&self) -> Result<Vec<CopyInventoryRow>, AmError>;

    /// The stored copy rows whose `request_id` has no durable request, ordered
    /// by copy id — the named open gap for a copy that was inventoried before
    /// its request landed, so a per-request read cannot silently drop it.
    ///
    /// # Errors
    /// Storage failures.
    fn open_gap_rows(&self) -> Result<Vec<CopyInventoryRow>, AmError>;
}

/// Refuse a row that is missing a required field.
fn validate_row(row: &CopyInventoryRow) -> Result<(), AmError> {
    for (name, value) in [
        ("copy_id", &row.copy_id),
        ("request_id", &row.request_id),
        ("custody", &row.custody),
        ("owner", &row.owner),
        ("required_action", &row.required_action),
        ("state", &row.state),
        ("updated_at", &row.updated_at),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("copy inventory {name} is required"),
            ));
        }
    }
    if row
        .exception_reason
        .as_deref()
        .is_some_and(|reason| reason.trim().is_empty())
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "copy inventory exception reason cannot be blank",
        ));
    }
    Ok(())
}

/// Refuse a re-save that binds a stored copy id to a different request or
/// custody.
fn identity_conflict(copy_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("copy `{copy_id}` is already stored with a different request or custody"),
    )
}

/// The row an insert must have left behind; losing it is a storage defect.
fn lost_row() -> AmError {
    AmError::new(
        ErrorCode::Internal,
        "copy inventory insert left no row behind",
    )
}

const SQLITE_COLUMNS: &str = "copy_id, request_id, custody, owner, required_action, state, exception_reason, \
     updated_at FROM copy_inventory";

fn sqlite_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CopyInventoryRow> {
    Ok(CopyInventoryRow {
        copy_id: row.get(0)?,
        request_id: row.get(1)?,
        custody: row.get(2)?,
        owner: row.get(3)?,
        required_action: row.get(4)?,
        state: row.get(5)?,
        exception_reason: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn sqlite_read(
    conn: &rusqlite::Connection,
    copy_id: &str,
) -> Result<Option<CopyInventoryRow>, AmError> {
    let sql = format!("SELECT {SQLITE_COLUMNS} WHERE copy_id = ?1");
    conn.query_row(&sql, rusqlite::params![copy_id], sqlite_row)
        .optional()
        .am("copy inventory read")
}

impl CopyInventoryStore for SqliteStore {
    fn save(&self, row: &CopyInventoryRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_conn(|conn| {
            converge_copy_inventory_sqlite(conn)?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .am("copy inventory begin")?;
            tx.execute(
                "INSERT INTO copy_inventory
                    (copy_id, request_id, custody, owner, required_action, state, \
                     exception_reason, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (copy_id) DO NOTHING",
                rusqlite::params![
                    row.copy_id,
                    row.request_id,
                    row.custody,
                    row.owner,
                    row.required_action,
                    row.state,
                    row.exception_reason,
                    row.updated_at
                ],
            )
            .am("copy inventory insert")?;
            let stored = sqlite_read(&tx, &row.copy_id)?.ok_or_else(lost_row)?;
            if stored != *row {
                if stored.request_id != row.request_id || stored.custody != row.custody {
                    return Err(identity_conflict(&row.copy_id));
                }
                tx.execute(
                    "UPDATE copy_inventory
                     SET owner = ?1, required_action = ?2, state = ?3, \
                         exception_reason = ?4, updated_at = ?5
                     WHERE copy_id = ?6",
                    rusqlite::params![
                        row.owner,
                        row.required_action,
                        row.state,
                        row.exception_reason,
                        row.updated_at,
                        row.copy_id
                    ],
                )
                .am("copy inventory update")?;
            }
            tx.commit().am("copy inventory commit")
        })
    }

    fn load(&self, copy_id: &str) -> Result<Option<CopyInventoryRow>, AmError> {
        self.with_conn(|conn| {
            converge_copy_inventory_sqlite(conn)?;
            sqlite_read(conn, copy_id)
        })
    }

    fn load_all(&self) -> Result<Vec<CopyInventoryRow>, AmError> {
        self.with_conn(|conn| {
            converge_copy_inventory_sqlite(conn)?;
            let sql = format!("SELECT {SQLITE_COLUMNS} ORDER BY copy_id");
            let mut stmt = conn.prepare(&sql).am("copy inventory list prepare")?;
            let rows = stmt
                .query_map([], sqlite_row)
                .am("copy inventory list rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("copy inventory list rows")
        })
    }

    fn open_gap_rows(&self) -> Result<Vec<CopyInventoryRow>, AmError> {
        self.with_conn(|conn| {
            converge_copy_inventory_sqlite(conn)?;
            let mut stmt = conn
                .prepare(
                    "SELECT copy_id, request_id, custody, owner, required_action, state, \
                            exception_reason, updated_at
                     FROM copy_inventory
                     WHERE NOT EXISTS (
                         SELECT 1 FROM durable_requests
                         WHERE durable_requests.request_id = copy_inventory.request_id
                     )
                     ORDER BY copy_id",
                )
                .am("copy inventory open gap prepare")?;
            let rows = stmt
                .query_map([], sqlite_row)
                .am("copy inventory open gap rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("copy inventory open gap rows")
        })
    }
}

impl CopyInventoryStore for PostgresStore {
    fn save(&self, row: &CopyInventoryRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_client(|client| {
            converge_pg(client)?;
            let mut tx = client.transaction().am("copy inventory begin")?;
            tx.execute(
                "INSERT INTO copy_inventory
                    (copy_id, request_id, custody, owner, required_action, state, \
                     exception_reason, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (copy_id) DO NOTHING",
                &[
                    &row.copy_id,
                    &row.request_id,
                    &row.custody,
                    &row.owner,
                    &row.required_action,
                    &row.state,
                    &row.exception_reason,
                    &row.updated_at,
                ],
            )
            .am("copy inventory insert")?;
            let stored = tx
                .query_opt(
                    "SELECT copy_id, request_id, custody, owner, required_action, state, \
                            exception_reason, updated_at
                     FROM copy_inventory WHERE copy_id = $1",
                    &[&row.copy_id],
                )
                .am("copy inventory read")?
                .map(|row| pg_row(&row))
                .ok_or_else(lost_row)?;
            if stored != *row {
                if stored.request_id != row.request_id || stored.custody != row.custody {
                    return Err(identity_conflict(&row.copy_id));
                }
                tx.execute(
                    "UPDATE copy_inventory
                     SET owner = $1, required_action = $2, state = $3, \
                         exception_reason = $4, updated_at = $5
                     WHERE copy_id = $6",
                    &[
                        &row.owner,
                        &row.required_action,
                        &row.state,
                        &row.exception_reason,
                        &row.updated_at,
                        &row.copy_id,
                    ],
                )
                .am("copy inventory update")?;
            }
            tx.commit().am("copy inventory commit")
        })
    }

    fn load(&self, copy_id: &str) -> Result<Option<CopyInventoryRow>, AmError> {
        self.with_client(|client| {
            converge_pg(client)?;
            Ok(client
                .query_opt(
                    "SELECT copy_id, request_id, custody, owner, required_action, state, \
                            exception_reason, updated_at
                     FROM copy_inventory WHERE copy_id = $1",
                    &[&copy_id],
                )
                .am("copy inventory read")?
                .map(|row| pg_row(&row)))
        })
    }

    fn load_all(&self) -> Result<Vec<CopyInventoryRow>, AmError> {
        self.with_client(|client| {
            converge_pg(client)?;
            let rows = client
                .query(
                    "SELECT copy_id, request_id, custody, owner, required_action, state, \
                            exception_reason, updated_at
                     FROM copy_inventory ORDER BY copy_id",
                    &[],
                )
                .am("copy inventory list rows")?;
            Ok(rows.iter().map(pg_row).collect())
        })
    }

    fn open_gap_rows(&self) -> Result<Vec<CopyInventoryRow>, AmError> {
        self.with_client(|client| {
            converge_pg(client)?;
            let rows = client
                .query(
                    "SELECT copy_id, request_id, custody, owner, required_action, state, \
                            exception_reason, updated_at
                     FROM copy_inventory
                     WHERE NOT EXISTS (
                         SELECT 1 FROM durable_requests
                         WHERE durable_requests.request_id = copy_inventory.request_id
                     )
                     ORDER BY copy_id",
                    &[],
                )
                .am("copy inventory open gap rows")?;
            Ok(rows.iter().map(pg_row).collect())
        })
    }
}

fn pg_row(row: &postgres::Row) -> CopyInventoryRow {
    CopyInventoryRow {
        copy_id: row.get(0),
        request_id: row.get(1),
        custody: row.get(2),
        owner: row.get(3),
        required_action: row.get(4),
        state: row.get(5),
        exception_reason: row.get(6),
        updated_at: row.get(7),
    }
}

#[cfg(test)]
#[path = "copy_inventory_tests.rs"]
mod copy_inventory_tests;

#[cfg(test)]
#[path = "copy_inventory_pg_tests.rs"]
mod copy_inventory_pg_tests;
