// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging a pre-existing placeholder
//! `local_events` table, which holds only a legacy `id` key, onto the event
//! columns the local event store writes, on both backends.
//!
//! ## Owns
//! - The placeholder-shape detection for both backends
//! - The additive event-column repair and the legacy `id` NOT NULL repair
//! - The unique guarantee on `action_id` and `seq` a repaired table needs
//!
//! ## Does not own
//! - The `local_events` slot DDL text (loaded through `migrate`)
//! - The event write and read contract (`local_events`)
//! - The store connection helpers (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how a placeholder `local_events` table is repaired should
//! normally require changes here. Changes to which columns the store writes
//! should not require changes here.
//!
//! Keep this module focused on placeholder convergence.

use crate::migrate::IntoAmError;
use clio_types::AmError;

/// The event columns a placeholder table converges onto, with the type each is
/// added as, in store write order.
const EVENT_COLUMNS: [(&str, &str); 8] = [
    ("action_id", "text"),
    ("seq", "bigint"),
    ("actor", "text"),
    ("scope", "text"),
    ("kind", "text"),
    ("event_time", "text"),
    ("outcome_state", "text"),
    ("result", "text"),
];

/// The legacy key a placeholder `local_events` table carries, and the column
/// names the current shape must hold instead.
const LEGACY_KEY: &str = "id";

/// Add the event columns to a pre-existing `local_events` table and back the
/// two columns the store deduplicates and orders on with unique indexes.
///
/// The column list is read once. A database whose table is absent runs no DDL:
/// the portable DDL that follows declares the whole shape. A table that already
/// holds all eight event columns and no legacy `id` column runs no DDL either.
/// Every other table gets the columns it lacks plus both unique indexes. Those
/// index statements are idempotent, so they re-run on each open of a table that
/// still carries the legacy key.
///
/// The added columns stay nullable because SQLite only accepts a `NOT NULL`
/// added column with a constant default. A placeholder's `id` primary key is not
/// an `INTEGER` key, so SQLite already allows NULL there: the insert that never
/// names it stores NULL and the column stays in place. The DDL's `NOT NULL` and
/// `CHECK` constraints therefore remain accepted drift, while uniqueness on
/// `action_id` and `seq` is restored. Placeholder tables hold no event rows by
/// contract, so converged rows start empty and fill on write.
pub(crate) fn converge_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    let Some(columns) = sqlite_columns(conn)? else {
        return Ok(());
    };
    let current = !columns.iter().any(|name| name == LEGACY_KEY)
        && EVENT_COLUMNS
            .iter()
            .all(|(column, _)| columns.iter().any(|name| name == column));
    if current {
        return Ok(());
    }
    for (column, sql_type) in EVENT_COLUMNS {
        crate::schema_converge::converge_column_sqlite(conn, "local_events", column, sql_type)?;
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS local_events_action_id_uidx
         ON local_events (action_id)",
        [],
    )
    .am("local events action-id index")?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS local_events_seq_uidx
         ON local_events (seq)",
        [],
    )
    .am("local events order-position index")?;
    Ok(())
}

/// The column names of a `local_events` table, or `None` when the table is
/// absent.
fn sqlite_columns(conn: &rusqlite::Connection) -> Result<Option<Vec<String>>, AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'local_events'",
            [],
            |r| r.get(0),
        )
        .am("local events table exists for convergence")?;
    if exists == 0 {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare("PRAGMA table_info(local_events)")
        .am("local events table_info for convergence")?;
    let present = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .am("local events table_info rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("local events table_info rows for convergence")?;
    Ok(Some(present))
}

/// The shape facts the Postgres convergence decision needs: how many columns
/// `local_events` has at all, how many of them are event columns, and whether
/// the legacy `id` key is still there as `NOT NULL`. `None` means the table
/// is absent, which a fresh database relies on: the DDL batch that follows
/// creates the whole shape.
fn pg_shape(client: &mut postgres::Client) -> Result<Option<(i64, bool)>, AmError> {
    let row = client
        .query_one(
            "SELECT
                 count(*),
                 count(*) FILTER (WHERE column_name IN (
                     'action_id', 'seq', 'actor', 'scope',
                     'kind', 'event_time', 'outcome_state', 'result'
                 )),
                 count(*) FILTER (WHERE column_name = 'id' AND is_nullable = 'NO')
             FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'local_events'",
            &[],
        )
        .am("local events shape check")?;
    if row.get::<_, i64>(0) == 0 {
        return Ok(None);
    }
    Ok(Some((row.get::<_, i64>(1), row.get::<_, i64>(2) > 0)))
}

/// The name of the `local_events` primary key, or `None` when the table has
/// none. The name is read from the catalog because a placeholder may have been
/// created with any key name.
fn pg_primary_key(client: &mut postgres::Client) -> Result<Option<String>, AmError> {
    let rows = client
        .query(
            "SELECT conname FROM pg_constraint
             WHERE conrelid = to_regclass('local_events') AND contype = 'p'",
            &[],
        )
        .am("local events primary key name")?;
    Ok(rows.first().map(|row| row.get::<_, String>(0)))
}

/// Bring a pre-existing placeholder `local_events` table onto the event
/// columns.
///
/// The shape is read first, so a database with no table and a table that already
/// holds all eight event columns and no legacy `id` key both run no DDL. A
/// placeholder also carries `id text PRIMARY KEY`, which Postgres makes
/// `NOT NULL`: the repair drops that key — by the name read from the catalog,
/// and only when a primary key exists — then drops the column's `NOT NULL`,
/// keeping the column and any value it holds. Uniqueness on `action_id` and
/// `seq` is restored with unique indexes. The DDL's `NOT NULL` and `CHECK`
/// constraints remain accepted drift. The slot DDL is portable across both
/// backends, so Postgres cannot carry these repairs in the shared slot file.
pub(crate) fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    let Some((event_columns, legacy_key)) = pg_shape(client)? else {
        return Ok(());
    };
    if event_columns == EVENT_COLUMNS.len() as i64 && !legacy_key {
        return Ok(());
    }
    let mut statements: Vec<String> = Vec::new();
    if legacy_key {
        // `ALTER COLUMN` errors when the column is absent, so these statements
        // run only on the column state just read.
        if let Some(name) = pg_primary_key(client)? {
            statements.push(format!(
                "ALTER TABLE local_events DROP CONSTRAINT IF EXISTS \"{}\";",
                name.replace('"', "\"\"")
            ));
        }
        statements.push("ALTER TABLE local_events ALTER COLUMN id DROP NOT NULL;".to_owned());
    }
    for (column, sql_type) in EVENT_COLUMNS {
        statements.push(format!(
            "ALTER TABLE local_events ADD COLUMN IF NOT EXISTS {column} {sql_type};"
        ));
    }
    statements.push(
        "CREATE UNIQUE INDEX IF NOT EXISTS local_events_action_id_uidx
         ON local_events (action_id);"
            .to_owned(),
    );
    statements.push(
        "CREATE UNIQUE INDEX IF NOT EXISTS local_events_seq_uidx
         ON local_events (seq);"
            .to_owned(),
    );
    client
        .batch_execute(&statements.join("\n"))
        .am("local events converge")
}

#[cfg(test)]
#[path = "local_events_converge_tests.rs"]
mod local_events_converge_tests;

#[cfg(test)]
#[path = "local_events_converge_pg_tests.rs"]
mod local_events_converge_pg_tests;
