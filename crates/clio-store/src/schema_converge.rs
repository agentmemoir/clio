// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging pre-existing SQLite tables onto
//! current additive columns before portable DDL runs: the `items` correction
//! lineage columns (`corrects_item_id`, `superseded_by_id`), the sealed
//! domain-record `context_cipher` carriers (`beliefs`, `persona_stable_entries`,
//! `persona_preferences`), and the preference/belief context effective-time
//! columns (`persona_preferences.context_as_of`, `beliefs.context_as_of`).
//!
//! ## Owns
//! - Guarded `ALTER TABLE … ADD COLUMN` for the lineage columns (SQLite)
//! - Guarded `ALTER TABLE … ADD COLUMN` for the domain-record `context_cipher`
//!   columns and the `context_as_of` effective-time columns (SQLite)
//!
//! ## Does not own
//! - Fresh-database column declaration (`sql/001_core.sql`)
//! - Postgres convergence (`sql/002_vectors_postgres.sql` `ADD COLUMN IF NOT EXISTS`)
//! - Schema-version stamping (`schema_reshape`)
//!
//! ## Boundary
//!
//! Changes to the SQLite in-place lineage convergence should normally require
//! changes here. Changes to schema-version stamping should not require changes here.
//!
//! Keep this module focused on the additive SQLite lineage-column convergence.

use crate::migrate::IntoAmError;
use clio_types::AmError;

/// Add the correction-lineage columns to an existing SQLite `items` table.
///
/// Fresh databases are untouched: when `items` is absent the portable DDL
/// declares the columns itself. SQLite has no `ADD COLUMN IF NOT EXISTS`, so
/// each column is checked against `PRAGMA table_info` first; a database that
/// already has both columns is a no-op.
pub(crate) fn converge_lineage_columns_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    converge_column_sqlite(conn, "items", "corrects_item_id", "text")?;
    converge_column_sqlite(conn, "items", "superseded_by_id", "text")
}

/// Add the sealed domain-record `context_cipher` columns and the preference
/// and belief `context_as_of` effective-time columns to existing SQLite
/// tables. Fresh databases are untouched; a database that already has a column
/// is a no-op. The Postgres convergence for the same columns lives in
/// `sql/002_vectors_postgres.sql` (`ADD COLUMN IF NOT EXISTS`).
pub(crate) fn converge_context_columns_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    converge_column_sqlite(conn, "beliefs", "context_cipher", "text")?;
    converge_column_sqlite(conn, "persona_stable_entries", "context_cipher", "text")?;
    converge_column_sqlite(conn, "persona_preferences", "context_cipher", "text")?;
    converge_column_sqlite(conn, "persona_preferences", "context_as_of", "text")?;
    converge_column_sqlite(conn, "beliefs", "context_as_of", "text")
}

/// Add the distilled-unit reconciliation-state and evidence columns to an
/// existing SQLite table: the `revision` column (`integer NOT NULL DEFAULT 1`,
/// a constant default SQLite's ADD COLUMN accepts), the three stale columns, and
/// `evidence_json` (`text NOT NULL DEFAULT '[]'`, likewise a constant). Fresh
/// databases are untouched; a database that already has a column is a no-op.
/// Pre-v2 rows read revision 1 and no stale state; pre-v3 rows read an empty
/// evidence trail.
pub(crate) fn converge_distilled_state_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    converge_column_sqlite(
        conn,
        "distilled_units",
        "revision",
        "integer NOT NULL DEFAULT 1",
    )?;
    converge_column_sqlite(conn, "distilled_units", "stale_reason", "text")?;
    converge_column_sqlite(conn, "distilled_units", "stale_detail", "text")?;
    converge_column_sqlite(conn, "distilled_units", "stale_at", "text")?;
    converge_column_sqlite(
        conn,
        "distilled_units",
        "evidence_json",
        "text NOT NULL DEFAULT '[]'",
    )
}

/// Guarded single-column convergence: no-op when the table is absent (fresh
/// database; portable DDL declares the column itself) or the column exists.
/// `ddl` is the full added-column definition (type, default, constraints).
fn converge_column_sqlite(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
    ddl: &str,
) -> Result<(), AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )
        .am("sqlite table exists for column convergence")?;
    if exists == 0 {
        return Ok(());
    }
    let mut has_column = false;
    {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .am("table_info for column convergence")?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .am("table_info rows for column convergence")?;
        for name in names {
            if name.am("table_info name for column convergence")?.as_str() == column {
                has_column = true;
            }
        }
    }
    if !has_column {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
            [],
        )
        .am("add converged column")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "schema_converge_tests.rs"]
mod schema_converge_tests;
