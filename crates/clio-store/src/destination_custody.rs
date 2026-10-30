// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for export destination custody persistence: one
//! row per export action, holding the destination path, owner, required
//! action, pending delivery state, and the copy-custody digest and receiver
//! on both backends.
//!
//! ## Owns
//! - `DestinationCustodyStore`, the save/mark/load contract, and
//!   `DestinationCustodyRow`
//! - The per-backend SQL for the pending insert, the delivered and failed
//!   marks, the read-backs, and the pending-open read (both drivers, here)
//!
//! ## Does not own
//! - The `export_destinations` DDL text (loaded through `migrate`)
//! - The file staging and destination write that consume a custody row
//! - The delivery outcome vocabulary beyond the stored state text
//!
//! ## Boundary
//!
//! Changes to which columns the store writes or reads should normally require
//! changes here. Changes to how an export is delivered should not require
//! changes here.
//!
//! Keep this module focused on the export destination custody contract.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::OptionalExtension;

/// One export destination custody row, as saved and read back.
///
/// The row records where an export will land, who owns the copy, the action
/// the destination requires, and the delivery state. When a copy-custody
/// manifest backs the copy, it also records the manifest's copy digest and
/// receiver identity. It carries no export content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationCustodyRow {
    /// The export action id; unique across all stored destinations.
    pub action_id: String,
    /// The destination path the export will be written to.
    pub destination_path: String,
    /// The owner of the destination copy.
    pub custody_owner: String,
    /// The action the destination requires before it is settled.
    pub required_action: String,
    /// The delivery state: `pending`, `delivered`, or `failed`.
    pub state: String,
    /// Why the delivery failed, when it did.
    pub exception_reason: Option<String>,
    /// The copy digest the custody manifest recorded, prefixed with its
    /// algorithm (`sha256:<hex>`); NULL until a delivery writes it.
    pub copy_digest: Option<String>,
    /// The receiver identity the custody manifest named; NULL until a
    /// delivery writes it.
    pub receiver: Option<String>,
    /// The last update time, RFC3339 UTC.
    pub updated_at: String,
}

/// Persistence contract for export destination custody.
///
/// Implementations MUST persist the pending row before the destination file is
/// written, so a crash leaves a durable pending row that reconciliation finds
/// by action id. A repeated pending save for an existing action id is refused
/// and changes nothing.
pub trait DestinationCustodyStore: Send + Sync {
    /// Save the pending row for a new export action.
    ///
    /// # Errors
    /// Blank required fields, an existing `action_id`, or storage failures.
    fn save_pending(&self, row: &DestinationCustodyRow) -> Result<(), AmError>;

    /// Mark the row for `action_id` delivered.
    ///
    /// # Errors
    /// No row for `action_id`, or storage failures.
    fn mark_delivered(&self, action_id: &str, updated_at: &str) -> Result<(), AmError>;

    /// Mark the row for `action_id` failed with a reason.
    ///
    /// # Errors
    /// No row for `action_id`, or storage failures.
    fn mark_failed(&self, action_id: &str, reason: &str, updated_at: &str) -> Result<(), AmError>;

    /// The stored row for `action_id`, if any.
    ///
    /// # Errors
    /// Storage failures.
    fn load(&self, action_id: &str) -> Result<Option<DestinationCustodyRow>, AmError>;

    /// Every stored row, ordered by action id.
    ///
    /// # Errors
    /// Storage failures.
    fn load_all(&self) -> Result<Vec<DestinationCustodyRow>, AmError>;

    /// The rows still in state `pending`, ordered by action id — the restart
    /// reconciliation queue.
    ///
    /// # Errors
    /// Storage failures.
    fn open_rows(&self) -> Result<Vec<DestinationCustodyRow>, AmError>;
}

/// Refuse a row that is missing a required field.
fn validate_row(row: &DestinationCustodyRow) -> Result<(), AmError> {
    for (name, value) in [
        ("action_id", &row.action_id),
        ("destination_path", &row.destination_path),
        ("custody_owner", &row.custody_owner),
        ("required_action", &row.required_action),
        ("state", &row.state),
        ("updated_at", &row.updated_at),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("export destination {name} is required"),
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
            "export destination exception reason cannot be blank",
        ));
    }
    for (name, value) in [
        ("copy_digest", row.copy_digest.as_deref()),
        ("receiver", row.receiver.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("export destination {name} cannot be blank"),
            ));
        }
    }
    Ok(())
}

/// Refuse a pending save for an action id that already has a row.
fn duplicate_action(action_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("export destination `{action_id}` already has a row"),
    )
}

/// Refuse a mark for an action id with no row.
fn unknown_action(action_id: &str) -> AmError {
    AmError::new(
        ErrorCode::NotFound,
        format!("no export destination row exists for action `{action_id}`"),
    )
}

fn validate_action_id(action_id: &str) -> Result<(), AmError> {
    if action_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "export destination action_id is required",
        ));
    }
    Ok(())
}

const SQLITE_COLUMNS: &str = "action_id, destination_path, custody_owner, required_action, state, \
     exception_reason, copy_digest, receiver, updated_at FROM export_destinations";

fn sqlite_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DestinationCustodyRow> {
    Ok(DestinationCustodyRow {
        action_id: row.get(0)?,
        destination_path: row.get(1)?,
        custody_owner: row.get(2)?,
        required_action: row.get(3)?,
        state: row.get(4)?,
        exception_reason: row.get(5)?,
        copy_digest: row.get(6)?,
        receiver: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn sqlite_read(
    conn: &rusqlite::Connection,
    action_id: &str,
) -> Result<Option<DestinationCustodyRow>, AmError> {
    let sql = format!("SELECT {SQLITE_COLUMNS} WHERE action_id = ?1");
    conn.query_row(&sql, rusqlite::params![action_id], sqlite_row)
        .optional()
        .am("export destination read")
}

impl DestinationCustodyStore for SqliteStore {
    fn save_pending(&self, row: &DestinationCustodyRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .am("export destination begin")?;
            let exists: Option<bool> = tx
                .query_row(
                    "SELECT 1 FROM export_destinations WHERE action_id = ?1",
                    rusqlite::params![row.action_id],
                    |r| r.get(0),
                )
                .optional()
                .am("export destination lookup")?;
            if exists.is_some() {
                return Err(duplicate_action(&row.action_id));
            }
            tx.execute(
                "INSERT INTO export_destinations
                    (action_id, destination_path, custody_owner, required_action, state, \
                     exception_reason, copy_digest, receiver, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    row.action_id,
                    row.destination_path,
                    row.custody_owner,
                    row.required_action,
                    row.state,
                    row.exception_reason,
                    row.copy_digest,
                    row.receiver,
                    row.updated_at
                ],
            )
            .am("export destination insert")?;
            tx.commit().am("export destination commit")
        })
    }

    fn mark_delivered(&self, action_id: &str, updated_at: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        self.with_conn(|conn| {
            let updated = conn
                .execute(
                    "UPDATE export_destinations
                     SET state = 'delivered', exception_reason = NULL, updated_at = ?1
                     WHERE action_id = ?2",
                    rusqlite::params![updated_at, action_id],
                )
                .am("export destination delivered update")?;
            if updated == 0 {
                return Err(unknown_action(action_id));
            }
            Ok(())
        })
    }

    fn mark_failed(&self, action_id: &str, reason: &str, updated_at: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        self.with_conn(|conn| {
            let updated = conn
                .execute(
                    "UPDATE export_destinations
                     SET state = 'failed', exception_reason = ?1, updated_at = ?2
                     WHERE action_id = ?3",
                    rusqlite::params![reason, updated_at, action_id],
                )
                .am("export destination failed update")?;
            if updated == 0 {
                return Err(unknown_action(action_id));
            }
            Ok(())
        })
    }

    fn load(&self, action_id: &str) -> Result<Option<DestinationCustodyRow>, AmError> {
        self.with_conn(|conn| sqlite_read(conn, action_id))
    }

    fn load_all(&self) -> Result<Vec<DestinationCustodyRow>, AmError> {
        self.with_conn(|conn| {
            let sql = format!("SELECT {SQLITE_COLUMNS} ORDER BY action_id");
            let mut stmt = conn.prepare(&sql).am("export destination list prepare")?;
            let rows = stmt
                .query_map([], sqlite_row)
                .am("export destination list rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("export destination list rows")
        })
    }

    fn open_rows(&self) -> Result<Vec<DestinationCustodyRow>, AmError> {
        self.with_conn(|conn| {
            let sql = format!("SELECT {SQLITE_COLUMNS} WHERE state = 'pending' ORDER BY action_id");
            let mut stmt = conn.prepare(&sql).am("export destination open prepare")?;
            let rows = stmt
                .query_map([], sqlite_row)
                .am("export destination open rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("export destination open rows")
        })
    }
}

fn pg_row(row: &postgres::Row) -> DestinationCustodyRow {
    DestinationCustodyRow {
        action_id: row.get(0),
        destination_path: row.get(1),
        custody_owner: row.get(2),
        required_action: row.get(3),
        state: row.get(4),
        exception_reason: row.get(5),
        copy_digest: row.get(6),
        receiver: row.get(7),
        updated_at: row.get(8),
    }
}

const PG_COLUMNS: &str = "action_id, destination_path, custody_owner, required_action, state, \
     exception_reason, copy_digest, receiver, updated_at FROM export_destinations";

impl DestinationCustodyStore for PostgresStore {
    fn save_pending(&self, row: &DestinationCustodyRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("export destination begin")?;
            let existing = tx
                .query_opt(
                    "SELECT 1 FROM export_destinations WHERE action_id = $1",
                    &[&row.action_id],
                )
                .am("export destination lookup")?;
            if existing.is_some() {
                return Err(duplicate_action(&row.action_id));
            }
            tx.execute(
                "INSERT INTO export_destinations
                    (action_id, destination_path, custody_owner, required_action, state, \
                     exception_reason, copy_digest, receiver, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &row.action_id,
                    &row.destination_path,
                    &row.custody_owner,
                    &row.required_action,
                    &row.state,
                    &row.exception_reason,
                    &row.copy_digest,
                    &row.receiver,
                    &row.updated_at,
                ],
            )
            .am("export destination insert")?;
            tx.commit().am("export destination commit")
        })
    }

    fn mark_delivered(&self, action_id: &str, updated_at: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        self.with_client(|client| {
            let updated = client
                .execute(
                    "UPDATE export_destinations
                     SET state = 'delivered', exception_reason = NULL, updated_at = $1
                     WHERE action_id = $2",
                    &[&updated_at, &action_id],
                )
                .am("export destination delivered update")?;
            if updated == 0 {
                return Err(unknown_action(action_id));
            }
            Ok(())
        })
    }

    fn mark_failed(&self, action_id: &str, reason: &str, updated_at: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        self.with_client(|client| {
            let updated = client
                .execute(
                    "UPDATE export_destinations
                     SET state = 'failed', exception_reason = $1, updated_at = $2
                     WHERE action_id = $3",
                    &[&reason, &updated_at, &action_id],
                )
                .am("export destination failed update")?;
            if updated == 0 {
                return Err(unknown_action(action_id));
            }
            Ok(())
        })
    }

    fn load(&self, action_id: &str) -> Result<Option<DestinationCustodyRow>, AmError> {
        self.with_client(|client| {
            Ok(client
                .query_opt(
                    &format!("SELECT {PG_COLUMNS} WHERE action_id = $1"),
                    &[&action_id],
                )
                .am("export destination read")?
                .map(|row| pg_row(&row)))
        })
    }

    fn load_all(&self) -> Result<Vec<DestinationCustodyRow>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(&format!("SELECT {PG_COLUMNS} ORDER BY action_id"), &[])
                .am("export destination list rows")?;
            Ok(rows.iter().map(pg_row).collect())
        })
    }

    fn open_rows(&self) -> Result<Vec<DestinationCustodyRow>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    &format!("SELECT {PG_COLUMNS} WHERE state = 'pending' ORDER BY action_id"),
                    &[],
                )
                .am("export destination open rows")?;
            Ok(rows.iter().map(pg_row).collect())
        })
    }
}

#[cfg(test)]
#[path = "destination_custody_tests.rs"]
mod destination_custody_tests;

#[cfg(test)]
#[path = "destination_custody_pg_tests.rs"]
mod destination_custody_pg_tests;
