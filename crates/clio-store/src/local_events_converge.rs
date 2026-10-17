// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging a pre-existing placeholder
//! `local_events` table, which holds only a legacy `id` key, onto the eight
//! event columns the local event store writes, on both backends.
//!
//! ## Owns
//! - The shape read that decides a table is already converged: its columns and
//!   the two unique guarantees
//! - The additive event-column repair and the removal of the legacy `id` column
//! - The deletion of a row whose eight event columns are all NULL
//! - The SQLite rebuild, run under one write transaction
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
//!
//! # The converged shape
//!
//! A repaired placeholder table ends with exactly the eight event columns, in
//! store write order, and nothing else. A unique single-column index backs
//! `action_id` and one backs `seq`: those are the guarantees the writer's
//! duplicate refusal and `ORDER BY seq` rest on. A partial, invalid, or
//! expression index is not such a guarantee. A backup compares its column list
//! with the target table's and refuses a column the target lacks, so a repaired
//! table must hold the same columns as a fresh one. A leftover `id` column makes
//! every backup of that store unrestorable.
//!
//! A table that already lost the legacy key is repaired in place: it gains the
//! columns and unique indexes it lacks, and it is never dropped and recreated.
//!
//! The DDL's `NOT NULL` and `CHECK` constraints stay absent, and that drift is
//! accepted. SQLite only accepts a `NOT NULL` added column with a constant
//! default, and no constant is right for a field the store always writes, so a
//! column added to an existing table is nullable. Adding a constraint to a table
//! that already holds rows can fail, which would fail the open it means to
//! repair.
//!
//! The SQLite rebuild could declare those constraints on its replacement table.
//! It does not. The rebuild copies rows that may predate the contract, so a
//! constraint would make the copy or the rename fail on real data, and dropping
//! those rows would delete them. The nullable columns and both unique indexes
//! are the repair.
//!
//! # Rows that carry no event
//!
//! A row whose eight event columns are all NULL is a leftover placeholder row:
//! the ordered read fails on its NULL `action_id`, so it would fail that read
//! forever. The repair deletes those rows only. A row with any event value is
//! kept.
//!
//! # Two first opens of one SQLite file
//!
//! The shape read and the repair are not one atomic step, so two opens of the
//! same legacy file can both reach the repair and collide on a duplicate column.
//! They meet on one write transaction: `BEGIN IMMEDIATE`, then the shape read
//! again, then the repair only if the table is still not converged. The driver
//! opens a connection with a five second busy timeout; the repair raises it to
//! ten, so the opener that loses the race waits for the winner instead of
//! failing at once.

use crate::migrate::IntoAmError;
use clio_types::AmError;
use rusqlite::TransactionBehavior;
use std::time::Duration;

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

/// The legacy key a placeholder `local_events` table carries, and the column the
/// converged shape must not hold.
const LEGACY_KEY: &str = "id";

/// The table the SQLite rebuild builds under, before it replaces the old one.
const REBUILD_TABLE: &str = "local_events_repaired";

/// How long the SQLite repair waits for another open's write lock.
const WRITE_LOCK_WAIT: Duration = Duration::from_secs(10);

/// The eight event column names, in store write order.
fn event_columns() -> Vec<&'static str> {
    EVENT_COLUMNS.iter().map(|(column, _)| *column).collect()
}

/// The column list every statement that names all eight event columns uses.
fn event_column_list() -> String {
    event_columns().join(", ")
}

/// The predicate that holds only for a row whose eight event columns are all
/// NULL, and so carries no event.
fn all_columns_null() -> String {
    event_columns()
        .iter()
        .map(|column| format!("{column} IS NULL"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The shape facts the SQLite repair decides on: the column names and whether a
/// unique single-column index backs `action_id` and `seq`.
struct SqliteShape {
    /// Every column of the table.
    columns: Vec<String>,
    /// A unique index over `action_id` alone exists.
    unique_action_id: bool,
    /// A unique index over `seq` alone exists.
    unique_seq: bool,
}

impl SqliteShape {
    /// Whether the table holds exactly the eight event columns and both unique
    /// guarantees, so the repair has nothing to do. Eight columns that are all
    /// event columns cannot include the legacy key.
    fn is_current(&self) -> bool {
        let expected = event_columns();
        self.columns.len() == expected.len()
            && expected
                .iter()
                .all(|column| self.columns.iter().any(|name| name == column))
            && self.unique_action_id
            && self.unique_seq
    }

    /// Whether the table still carries the legacy `id` key, which only a
    /// rebuild can remove.
    fn has_legacy_key(&self) -> bool {
        self.columns.iter().any(|name| name == LEGACY_KEY)
    }
}

/// Converge a pre-existing SQLite `local_events` table onto the event shape.
///
/// The shape is read first, so a database with no table and a table that already
/// holds the converged shape both run no DDL. A table that still carries the
/// legacy `id` key is rebuilt as exactly the eight nullable event columns and
/// both unique indexes: SQLite refuses to drop a `PRIMARY KEY` column, so the
/// key leaves with the table it belongs to. A table without the legacy key keeps
/// its shape and gains only what it lacks. Both repairs run in one write
/// transaction.
///
/// # Errors
/// Storage failures, including a write lock this connection could not take.
pub(crate) fn converge_sqlite(conn: &mut rusqlite::Connection) -> Result<(), AmError> {
    let Some(shape) = sqlite_shape(conn)? else {
        return Ok(());
    };
    if shape.is_current() {
        return Ok(());
    }
    conn.busy_timeout(WRITE_LOCK_WAIT)
        .am("local events write-lock wait")?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .am("local events converge lock")?;
    // Another open may have converged the table while this one waited for the
    // write lock, which is the whole point of the second read.
    match sqlite_shape(&tx)? {
        Some(shape) if !shape.is_current() && shape.has_legacy_key() => sqlite_rebuild(&tx)?,
        Some(shape) if !shape.is_current() => sqlite_additive(&tx)?,
        _ => {}
    }
    tx.commit().am("local events converge commit")
}

/// Add the event columns a table without the legacy key still lacks, delete a
/// row that carries no event, and create both unique indexes. Nothing here
/// rebuilds the table, so a table already close to the fresh shape keeps it.
fn sqlite_additive(tx: &rusqlite::Transaction<'_>) -> Result<(), AmError> {
    for (column, sql_type) in EVENT_COLUMNS {
        crate::schema_converge::converge_column_sqlite(tx, "local_events", column, sql_type)?;
    }
    tx.execute(
        &format!("DELETE FROM local_events WHERE {}", all_columns_null()),
        [],
    )
    .am("local events placeholder row delete")?;
    for (column, index) in [
        ("action_id", "local_events_action_id_uidx"),
        ("seq", "local_events_seq_uidx"),
    ] {
        tx.execute(
            &format!("CREATE UNIQUE INDEX IF NOT EXISTS {index} ON local_events ({column})"),
            [],
        )
        .am("local events unique index")?;
    }
    Ok(())
}

/// Rebuild `local_events` as exactly the eight nullable event columns.
///
/// Missing event columns are added first so the copy can name all eight. A row
/// whose eight event columns are all NULL is a leftover placeholder row and is
/// not copied; a row with any event value is. The replacement is dropped first
/// when a crashed attempt left it behind, so the rebuild can run again.
fn sqlite_rebuild(tx: &rusqlite::Transaction<'_>) -> Result<(), AmError> {
    for (column, sql_type) in EVENT_COLUMNS {
        crate::schema_converge::converge_column_sqlite(tx, "local_events", column, sql_type)?;
    }
    let declarations = EVENT_COLUMNS
        .iter()
        .map(|(column, sql_type)| format!("{column} {sql_type}"))
        .collect::<Vec<_>>()
        .join(", ");
    let columns = event_column_list();
    tx.execute_batch(&format!(
        "DROP TABLE IF EXISTS {REBUILD_TABLE};
         CREATE TABLE {REBUILD_TABLE} ({declarations});
         INSERT INTO {REBUILD_TABLE} ({columns})
             SELECT {columns} FROM local_events WHERE NOT ({});
         DROP TABLE local_events;
         ALTER TABLE {REBUILD_TABLE} RENAME TO local_events;
         CREATE UNIQUE INDEX local_events_action_id_uidx ON local_events (action_id);
         CREATE UNIQUE INDEX local_events_seq_uidx ON local_events (seq);",
        all_columns_null()
    ))
    .am("local events rebuild")
}

/// The `local_events` shape, or `None` when the table is absent.
fn sqlite_shape(conn: &rusqlite::Connection) -> Result<Option<SqliteShape>, AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'local_events'",
            [],
            |row| row.get(0),
        )
        .am("local events table exists for convergence")?;
    if exists == 0 {
        return Ok(None);
    }
    let mut table_info = conn
        .prepare("PRAGMA table_info(local_events)")
        .am("local events table_info for convergence")?;
    let columns = table_info
        .query_map([], |row| row.get::<_, String>(1))
        .am("local events table_info rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("local events table_info rows for convergence")?;
    drop(table_info);
    // A fresh table's primary key and UNIQUE constraint each appear as an
    // automatic unique index over one column; the repair creates named ones.
    let mut indexes = conn
        .prepare(
            "SELECT max(ii.name)
             FROM pragma_index_list('local_events') AS il
             JOIN pragma_index_info(il.name) AS ii
             WHERE il.\"unique\" = 1 AND il.partial = 0
             GROUP BY il.name
             HAVING count(*) = 1",
        )
        .am("local events unique index columns for convergence")?;
    // An expression index reports no column name here, so it never counts.
    let unique = indexes
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .am("local events unique index rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("local events unique index rows for convergence")?;
    Ok(Some(SqliteShape {
        columns,
        unique_action_id: unique.iter().flatten().any(|column| column == "action_id"),
        unique_seq: unique.iter().flatten().any(|column| column == "seq"),
    }))
}

/// The shape facts the Postgres repair decides on: how many event columns the
/// table holds, whether the legacy key is still a column, and whether a unique
/// single-column index backs `action_id` and `seq`.
struct PgShape {
    /// How many of the eight event columns the table holds.
    event_columns: i64,
    /// The legacy key is still a column, whatever its nullability.
    legacy_key: bool,
    /// A unique index over `action_id` alone exists.
    unique_action_id: bool,
    /// A unique index over `seq` alone exists.
    unique_seq: bool,
}

impl PgShape {
    /// Whether the table holds all eight event columns, no legacy key, and both
    /// unique guarantees, so the repair has nothing to do.
    fn is_current(&self) -> bool {
        self.event_columns == EVENT_COLUMNS.len() as i64
            && !self.legacy_key
            && self.unique_action_id
            && self.unique_seq
    }
}

/// The `local_events` shape, or `None` when the table is absent, which a fresh
/// database relies on: the DDL batch that follows creates the whole shape.
fn pg_shape(client: &mut postgres::Client) -> Result<Option<PgShape>, AmError> {
    let names = event_columns();
    let row = client
        .query_one(
            "SELECT
                 count(*),
                 count(*) FILTER (WHERE column_name = ANY($1)),
                 count(*) FILTER (WHERE column_name = 'id')
             FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'local_events'",
            &[&names],
        )
        .am("local events shape check")?;
    if row.get::<_, i64>(0) == 0 {
        return Ok(None);
    }
    let unique = client
        .query(
            "SELECT a.attname
             FROM pg_index AS i
             JOIN pg_attribute AS a
               ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
             WHERE i.indrelid = to_regclass('local_events')
               AND i.indisunique
               AND i.indisvalid
               AND i.indpred IS NULL
               AND i.indexprs IS NULL
               AND i.indnkeyatts = 1",
            &[],
        )
        .am("local events unique index check")?
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<Vec<_>>();
    Ok(Some(PgShape {
        event_columns: row.get(1),
        legacy_key: row.get::<_, i64>(2) > 0,
        unique_action_id: unique.iter().any(|column| column == "action_id"),
        unique_seq: unique.iter().any(|column| column == "seq"),
    }))
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

/// Converge a pre-existing Postgres `local_events` table onto the event shape.
///
/// The shape is read first, so a database with no table and a table that already
/// holds the converged shape both run no DDL. A table with the legacy `id` key
/// loses that key and its column. Every repaired table gains the event columns
/// it lacks, loses any row that carries no event, and gains both unique indexes.
/// The statements are idempotent, so a table that still needs one of them can be
/// repaired again.
///
/// # Errors
/// Storage failures, including a value that keeps a unique index from building.
pub(crate) fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    let Some(shape) = pg_shape(client)? else {
        return Ok(());
    };
    if shape.is_current() {
        return Ok(());
    }
    let mut statements: Vec<String> = Vec::new();
    if shape.legacy_key {
        // A primary key cannot be dropped by a name this code guessed, so the
        // name comes from the catalog; dropping the key first frees the column.
        // A table without the legacy key keeps the key it already declared.
        if let Some(name) = pg_primary_key(client)? {
            statements.push(format!(
                "ALTER TABLE local_events DROP CONSTRAINT IF EXISTS \"{}\";",
                name.replace('"', "\"\"")
            ));
        }
        statements.push(format!(
            "ALTER TABLE local_events DROP COLUMN IF EXISTS {LEGACY_KEY};"
        ));
    }
    for (column, sql_type) in EVENT_COLUMNS {
        statements.push(format!(
            "ALTER TABLE local_events ADD COLUMN IF NOT EXISTS {column} {sql_type};"
        ));
    }
    // After the columns exist: only a row whose eight event columns are all NULL
    // is the leftover placeholder row.
    statements.push(format!(
        "DELETE FROM local_events WHERE {};",
        all_columns_null()
    ));
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
