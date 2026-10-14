// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for durable subject-request record persistence:
//! one row per request on both backends, saved and read back as a
//! consumer-serialized record, with the immutable scope and kind checked
//! inside the write transaction.
//!
//! ## Owns
//! - `DurableRequestStore`, the save/load contract, and `DurableRequestRow`
//! - The per-backend SQL for the insert-or-refuse save and the restart reads
//!   (both drivers, in this module)
//!
//! ## Does not own
//! - The `durable_requests` DDL text (`sql/003_local_compliance_slots.sql`,
//!   loaded through `migrate`)
//! - The request vocabulary and its decoding: `kind`, `state`, `scope`, and
//!   `record` are the consumer's canonical serializations, stored verbatim
//! - The request state machine, restart reconciliation, and any auto-completion
//!
//! ## Boundary
//!
//! Changes to which columns the store writes or reads should normally require
//! changes here. Changes to how a request's state moves should not require
//! changes here.
//!
//! Keep this module focused on the durable request persistence contract.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::schema_converge::converge_durable_requests_sqlite;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::OptionalExtension;

/// One durable request row, as saved and read back.
///
/// clio-store holds the record without decoding it: the consumer serializes
/// the request into `record` (and its scope into `scope`) in one canonical
/// form, and the store persists and compares those values verbatim. `kind`,
/// `state`, and `deadline` are normalized columns for query and restart use;
/// every other field the consumer needs — per-path state with its saved
/// attempt and action ids, actions, peers, exception — lives inside `record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRequestRow {
    /// The durable request id; unique across all stored requests.
    pub request_id: String,
    /// The request kind, fixed when the request is opened.
    pub kind: String,
    /// The current request state, including the final outcome.
    pub state: String,
    /// The canonical scope serialization; fixed when the request is opened.
    pub scope: String,
    /// The operator-supplied deadline, once recorded.
    pub deadline: Option<String>,
    /// The canonical full-record serialization.
    pub record: String,
}

/// Persistence contract for durable subject requests.
///
/// Implementations MUST check the stored scope and kind inside the write
/// transaction, so a reused id bound to a different scope or kind is refused
/// without mutating anything, even when two writers race. A restart reads
/// rows back verbatim: nothing is auto-completed or auto-cancelled.
pub trait DurableRequestStore: Send + Sync {
    /// Save one request record.
    ///
    /// The first save for a `request_id` inserts the row. A re-save of the
    /// identical row is a no-op, so a retried write after a restart cannot
    /// double-apply. A re-save with the same scope and kind updates the row;
    /// one with a different scope or kind is refused and changes nothing.
    ///
    /// # Errors
    /// Blank required fields, a reused id bound to a different scope or kind,
    /// or storage failures.
    fn save(&self, row: &DurableRequestRow) -> Result<(), AmError>;

    /// The stored row for `request_id`, if any.
    ///
    /// # Errors
    /// Storage failures.
    fn load(&self, request_id: &str) -> Result<Option<DurableRequestRow>, AmError>;

    /// Every stored row, ordered by request id — the restart read.
    ///
    /// # Errors
    /// Storage failures.
    fn load_all(&self) -> Result<Vec<DurableRequestRow>, AmError>;
}

/// Refuse a row that is missing a required field.
fn validate_row(row: &DurableRequestRow) -> Result<(), AmError> {
    for (name, value) in [
        ("request_id", &row.request_id),
        ("kind", &row.kind),
        ("state", &row.state),
        ("scope", &row.scope),
        ("record", &row.record),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("durable request {name} is required"),
            ));
        }
    }
    if row
        .deadline
        .as_deref()
        .is_some_and(|deadline| deadline.trim().is_empty())
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "durable request deadline cannot be blank",
        ));
    }
    Ok(())
}

/// Refuse a re-save that binds a stored id to a different scope or kind.
fn identity_conflict(request_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("durable request `{request_id}` is already stored with a different scope or kind"),
    )
}

/// The row an insert must have left behind; losing it is a storage defect.
fn lost_row() -> AmError {
    AmError::new(
        ErrorCode::Internal,
        "durable request insert left no row behind",
    )
}

const SQLITE_COLUMNS: &str =
    "request_id, kind, state, scope, deadline, record FROM durable_requests";

/// Bring a pre-existing single-key placeholder `durable_requests` table onto
/// the request columns (`ADD COLUMN IF NOT EXISTS`, so a current table is a
/// no-op). The slot DDL is portable across both backends, so Postgres cannot
/// carry these in the shared slot file the way the vector file does.
fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    client
        .batch_execute(
            "ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS request_id text;
             ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS kind text;
             ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS state text;
             ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS scope text;
             ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS deadline text;
             ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS record text;
             CREATE UNIQUE INDEX IF NOT EXISTS durable_requests_request_id_uidx
                 ON durable_requests (request_id);",
        )
        .am("durable request converge")
}

fn sqlite_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableRequestRow> {
    Ok(DurableRequestRow {
        request_id: row.get(0)?,
        kind: row.get(1)?,
        state: row.get(2)?,
        scope: row.get(3)?,
        deadline: row.get(4)?,
        record: row.get(5)?,
    })
}

fn sqlite_read(
    conn: &rusqlite::Connection,
    request_id: &str,
) -> Result<Option<DurableRequestRow>, AmError> {
    let sql = format!("SELECT {SQLITE_COLUMNS} WHERE request_id = ?1");
    conn.query_row(&sql, rusqlite::params![request_id], sqlite_row)
        .optional()
        .am("durable request read")
}

impl DurableRequestStore for SqliteStore {
    fn save(&self, row: &DurableRequestRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_conn(|conn| {
            converge_durable_requests_sqlite(conn)?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .am("durable request begin")?;
            tx.execute(
                "INSERT INTO durable_requests
                    (request_id, kind, state, scope, deadline, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (request_id) DO NOTHING",
                rusqlite::params![
                    row.request_id,
                    row.kind,
                    row.state,
                    row.scope,
                    row.deadline,
                    row.record
                ],
            )
            .am("durable request insert")?;
            let stored = sqlite_read(&tx, &row.request_id)?.ok_or_else(lost_row)?;
            if stored != *row {
                if stored.scope != row.scope || stored.kind != row.kind {
                    return Err(identity_conflict(&row.request_id));
                }
                tx.execute(
                    "UPDATE durable_requests
                     SET kind = ?1, state = ?2, deadline = ?3, record = ?4
                     WHERE request_id = ?5",
                    rusqlite::params![
                        row.kind,
                        row.state,
                        row.deadline,
                        row.record,
                        row.request_id
                    ],
                )
                .am("durable request update")?;
            }
            tx.commit().am("durable request commit")
        })
    }

    fn load(&self, request_id: &str) -> Result<Option<DurableRequestRow>, AmError> {
        self.with_conn(|conn| {
            converge_durable_requests_sqlite(conn)?;
            sqlite_read(conn, request_id)
        })
    }

    fn load_all(&self) -> Result<Vec<DurableRequestRow>, AmError> {
        self.with_conn(|conn| {
            converge_durable_requests_sqlite(conn)?;
            let sql = format!("SELECT {SQLITE_COLUMNS} ORDER BY request_id");
            let mut stmt = conn.prepare(&sql).am("durable request list prepare")?;
            let rows = stmt
                .query_map([], sqlite_row)
                .am("durable request list rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("durable request list rows")
        })
    }
}

impl DurableRequestStore for PostgresStore {
    fn save(&self, row: &DurableRequestRow) -> Result<(), AmError> {
        validate_row(row)?;
        self.with_client(|client| {
            converge_pg(client)?;
            let mut tx = client.transaction().am("durable request begin")?;
            tx.execute(
                "INSERT INTO durable_requests
                    (request_id, kind, state, scope, deadline, record)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (request_id) DO NOTHING",
                &[
                    &row.request_id,
                    &row.kind,
                    &row.state,
                    &row.scope,
                    &row.deadline,
                    &row.record,
                ],
            )
            .am("durable request insert")?;
            let stored = tx
                .query_opt(
                    "SELECT request_id, kind, state, scope, deadline, record
                     FROM durable_requests WHERE request_id = $1",
                    &[&row.request_id],
                )
                .am("durable request read")?
                .map(|row| pg_row(&row))
                .ok_or_else(lost_row)?;
            if stored != *row {
                if stored.scope != row.scope || stored.kind != row.kind {
                    return Err(identity_conflict(&row.request_id));
                }
                tx.execute(
                    "UPDATE durable_requests
                     SET kind = $1, state = $2, deadline = $3, record = $4
                     WHERE request_id = $5",
                    &[
                        &row.kind,
                        &row.state,
                        &row.deadline,
                        &row.record,
                        &row.request_id,
                    ],
                )
                .am("durable request update")?;
            }
            tx.commit().am("durable request commit")
        })
    }

    fn load(&self, request_id: &str) -> Result<Option<DurableRequestRow>, AmError> {
        self.with_client(|client| {
            converge_pg(client)?;
            Ok(client
                .query_opt(
                    "SELECT request_id, kind, state, scope, deadline, record
                     FROM durable_requests WHERE request_id = $1",
                    &[&request_id],
                )
                .am("durable request read")?
                .map(|row| pg_row(&row)))
        })
    }

    fn load_all(&self) -> Result<Vec<DurableRequestRow>, AmError> {
        self.with_client(|client| {
            converge_pg(client)?;
            let rows = client
                .query(
                    "SELECT request_id, kind, state, scope, deadline, record
                     FROM durable_requests ORDER BY request_id",
                    &[],
                )
                .am("durable request list rows")?;
            Ok(rows.iter().map(pg_row).collect())
        })
    }
}

fn pg_row(row: &postgres::Row) -> DurableRequestRow {
    DurableRequestRow {
        request_id: row.get(0),
        kind: row.get(1),
        state: row.get(2),
        scope: row.get(3),
        deadline: row.get(4),
        record: row.get(5),
    }
}

#[cfg(test)]
#[path = "durable_requests_tests.rs"]
mod durable_requests_tests;

#[cfg(test)]
#[path = "durable_requests_pg_tests.rs"]
mod durable_requests_pg_tests;
