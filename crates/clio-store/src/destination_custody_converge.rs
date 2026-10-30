// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging a pre-existing
//! `export_destinations` table onto the copy-custody columns (`copy_digest`,
//! `receiver`) on both backends.
//!
//! ## Owns
//! - The guarded additive column convergence for the two copy-custody columns
//!   (SQLite and Postgres)
//!
//! ## Does not own
//! - The `export_destinations` slot DDL text (loaded through `migrate`)
//! - The custody store contract (`destination_custody`)
//! - Other slot-table convergence (`local_events_converge`,
//!   `durable_requests_converge`, `copy_inventory_converge`)
//!
//! ## Boundary
//!
//! Changes to which columns a pre-existing `export_destinations` table gains
//! should normally require changes here. Changes to the columns the custody
//! store writes on a fresh table should not require changes here.
//!
//! Keep this module focused on the export-destination custody column
//! convergence.

use crate::migrate::IntoAmError;
use clio_types::AmError;

/// The copy-custody columns a pre-existing table gains, with their SQL types.
const CUSTODY_COLUMNS: [(&str, &str); 2] = [("copy_digest", "text"), ("receiver", "text")];

/// Add the copy-custody columns to an existing SQLite `export_destinations`
/// table.
///
/// Fresh databases are untouched: the table is absent before the slot DDL has
/// run, and that DDL declares the columns itself. A database that already has
/// a column is a no-op.
///
/// # Errors
/// Storage failures.
pub(crate) fn converge_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    for (column, sql_type) in CUSTODY_COLUMNS {
        crate::schema_converge::converge_column_sqlite(
            conn,
            "export_destinations",
            column,
            sql_type,
        )?;
    }
    Ok(())
}

/// Add the copy-custody columns to an existing Postgres `export_destinations`
/// table.
///
/// Fresh databases are untouched: the table is absent before the migrate batch
/// has run, and the slot DDL declares the columns itself. The statements are
/// idempotent, so a table that already holds a column can be converged again.
///
/// # Errors
/// Storage failures.
pub(crate) fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    let exists: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_name = 'export_destinations')",
            &[],
        )
        .am("export destination converge table probe")?
        .get(0);
    if !exists {
        return Ok(());
    }
    let mut statements: Vec<String> = Vec::new();
    for (column, sql_type) in CUSTODY_COLUMNS {
        statements.push(format!(
            "ALTER TABLE export_destinations ADD COLUMN IF NOT EXISTS {column} {sql_type};"
        ));
    }
    client
        .batch_execute(&statements.join("\n"))
        .am("export destination converge")
}

#[cfg(test)]
#[path = "destination_custody_converge_tests.rs"]
mod destination_custody_converge_tests;

#[cfg(test)]
#[path = "destination_custody_converge_pg_tests.rs"]
mod destination_custody_converge_pg_tests;
