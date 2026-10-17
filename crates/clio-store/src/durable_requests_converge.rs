// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging a pre-existing placeholder
//! `durable_requests` table, which holds only a legacy `id` key, onto the six
//! request columns the durable request store writes, on both backends.
//!
//! ## Owns
//! - The shape read that decides a table is already converged: its columns and
//!   the unique guarantee on `request_id`
//! - The additive request-column repair and the removal of the legacy `id`
//!   column
//! - The deletion of a row whose six request columns are all NULL
//! - The SQLite rebuild, run under one write transaction
//!
//! ## Does not own
//! - The `durable_requests` slot DDL text (loaded through `migrate`)
//! - The save/load contract and the row shape (`durable_requests`)
//! - The store connection helpers (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how a placeholder `durable_requests` table is repaired should
//! normally require changes here. Changes to which columns the store writes
//! should not require changes here.
//!
//! Keep this module focused on placeholder convergence.
//!
//! # The converged shape
//!
//! A repaired table ends with exactly the six request columns, in store write
//! order, and nothing else. A unique single-column index backs `request_id`: that
//! is the guarantee the save's conflict target and its duplicate refusal rest on,
//! and a partial, invalid, or expression index is not such a guarantee. A backup
//! compares its column list with the target table's and refuses a column the
//! target lacks, so a repaired table must hold the same columns as a fresh one. A
//! leftover `id` column makes every backup of that store unrestorable, and on
//! Postgres the primary key behind it makes the column NOT NULL, so every insert
//! that names the six request columns would be refused.
//!
//! A table that already lost the legacy key is repaired in place: it gains the
//! columns and unique index it lacks, and it is never dropped and recreated.
//!
//! The DDL's `NOT NULL` constraints stay absent, and that drift is accepted.
//! SQLite only accepts a `NOT NULL` added column with a constant default, and no
//! constant is right for a field the store always writes, so a column added to an
//! existing table is nullable. Adding a constraint to a table that already holds
//! rows can fail, which would fail the use it means to repair. The SQLite rebuild
//! could declare them on its replacement table; it does not, because it copies
//! rows that may predate the contract.
//!
//! # Rows that carry no request
//!
//! A row whose six request columns are all NULL is a leftover placeholder row: a
//! restart read cannot report it, so the repair deletes exactly those rows. A row
//! with any request value is kept.
//!
//! # Two first uses of one SQLite file
//!
//! The shape read and the repair are not one atomic step, so two first uses of
//! one legacy file can both reach the repair and collide on a duplicate column.
//! They meet on one write transaction: `BEGIN IMMEDIATE`, then the shape read
//! again, then the repair only if the table is still not converged. The driver
//! opens a connection with a five second busy timeout; the repair raises it to
//! ten, so the caller that loses the race waits for the winner instead of failing
//! at once.

use crate::migrate::IntoAmError;
use clio_types::AmError;
use rusqlite::TransactionBehavior;
use std::time::Duration;

/// The request columns a placeholder table converges onto, with the type each is
/// added as, in store write order.
const REQUEST_COLUMNS: [(&str, &str); 6] = [
    ("request_id", "text"),
    ("kind", "text"),
    ("state", "text"),
    ("scope", "text"),
    ("deadline", "text"),
    ("record", "text"),
];

/// The legacy key a placeholder `durable_requests` table carries, and the column
/// the converged shape must not hold.
const LEGACY_KEY: &str = "id";

/// The column the save's conflict target names, and the unique index that backs
/// it.
const KEY_COLUMN: &str = "request_id";
const UNIQUE_INDEX: &str = "durable_requests_request_id_uidx";

/// The table the SQLite rebuild builds under, before it replaces the old one.
const REBUILD_TABLE: &str = "durable_requests_repaired";

/// How long the SQLite repair waits for another writer's write lock.
const WRITE_LOCK_WAIT: Duration = Duration::from_secs(10);

/// The six request column names, in store write order.
fn request_columns() -> Vec<&'static str> {
    REQUEST_COLUMNS.iter().map(|(column, _)| *column).collect()
}

/// The column list every statement that names all six request columns uses.
fn request_column_list() -> String {
    request_columns().join(", ")
}

/// The predicate that holds only for a row whose six request columns are all
/// NULL, and so carries no request.
fn all_columns_null() -> String {
    request_columns()
        .iter()
        .map(|column| format!("{column} IS NULL"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The shape facts the SQLite repair decides on: the column names and whether a
/// unique single-column index backs `request_id`.
struct SqliteShape {
    /// Every column of the table.
    columns: Vec<String>,
    /// A unique index over `request_id` alone exists.
    unique_key: bool,
}

impl SqliteShape {
    /// Whether the table holds exactly the six request columns and the unique
    /// guarantee, so the repair has nothing to do. Six columns that are all
    /// request columns cannot include the legacy key.
    fn is_current(&self) -> bool {
        let expected = request_columns();
        self.columns.len() == expected.len()
            && expected
                .iter()
                .all(|column| self.columns.iter().any(|name| name == column))
            && self.unique_key
    }

    /// Whether the table still carries the legacy `id` key, which only a rebuild
    /// can remove.
    fn has_legacy_key(&self) -> bool {
        self.columns.iter().any(|name| name == LEGACY_KEY)
    }
}

/// Converge a pre-existing SQLite `durable_requests` table onto the request shape.
///
/// The shape is read first, so a database with no table and a table that already
/// holds the converged shape both run no DDL. A table that still carries the
/// legacy `id` key is rebuilt as exactly the six nullable request columns and the
/// unique index: SQLite refuses to drop a `PRIMARY KEY` column, so the key leaves
/// with the table it belongs to. A table without the legacy key keeps its shape
/// and gains only what it lacks. Both repairs run in one write transaction.
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
        .am("durable request write-lock wait")?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .am("durable request converge lock")?;
    // Another caller may have converged the table while this one waited for the
    // write lock, which is the whole point of the second read.
    match sqlite_shape(&tx)? {
        Some(shape) if !shape.is_current() && shape.has_legacy_key() => sqlite_rebuild(&tx)?,
        Some(shape) if !shape.is_current() => sqlite_additive(&tx)?,
        _ => {}
    }
    tx.commit().am("durable request converge commit")
}

/// Add the request columns a table without the legacy key still lacks, delete a
/// row that carries no request, and create the unique index. Nothing here
/// rebuilds the table, so a table already close to the fresh shape keeps it.
fn sqlite_additive(tx: &rusqlite::Transaction<'_>) -> Result<(), AmError> {
    for (column, sql_type) in REQUEST_COLUMNS {
        crate::schema_converge::converge_column_sqlite(tx, "durable_requests", column, sql_type)?;
    }
    tx.execute(
        &format!("DELETE FROM durable_requests WHERE {}", all_columns_null()),
        [],
    )
    .am("durable request placeholder row delete")?;
    tx.execute(
        &format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS {UNIQUE_INDEX} ON durable_requests ({KEY_COLUMN})"
        ),
        [],
    )
    .am("durable request unique index")?;
    Ok(())
}

/// Rebuild `durable_requests` as exactly the six nullable request columns.
///
/// Missing columns are added first so the copy can name all six. A row whose six
/// request columns are all NULL is a leftover placeholder row and is not copied;
/// a row with any request value is. The replacement is dropped first when a
/// crashed attempt left it behind, so the rebuild can run again.
fn sqlite_rebuild(tx: &rusqlite::Transaction<'_>) -> Result<(), AmError> {
    for (column, sql_type) in REQUEST_COLUMNS {
        crate::schema_converge::converge_column_sqlite(tx, "durable_requests", column, sql_type)?;
    }
    let declarations = REQUEST_COLUMNS
        .iter()
        .map(|(column, sql_type)| format!("{column} {sql_type}"))
        .collect::<Vec<_>>()
        .join(", ");
    let columns = request_column_list();
    tx.execute_batch(&format!(
        "DROP TABLE IF EXISTS {REBUILD_TABLE};
         CREATE TABLE {REBUILD_TABLE} ({declarations});
         INSERT INTO {REBUILD_TABLE} ({columns})
             SELECT {columns} FROM durable_requests WHERE NOT ({});
         DROP TABLE durable_requests;
         ALTER TABLE {REBUILD_TABLE} RENAME TO durable_requests;
         CREATE UNIQUE INDEX {UNIQUE_INDEX} ON durable_requests ({KEY_COLUMN});",
        all_columns_null()
    ))
    .am("durable request rebuild")
}

/// The `durable_requests` shape, or `None` when the table is absent.
fn sqlite_shape(conn: &rusqlite::Connection) -> Result<Option<SqliteShape>, AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'durable_requests'",
            [],
            |row| row.get(0),
        )
        .am("durable request table exists for convergence")?;
    if exists == 0 {
        return Ok(None);
    }
    let mut table_info = conn
        .prepare("PRAGMA table_info(durable_requests)")
        .am("durable request table_info for convergence")?;
    let columns = table_info
        .query_map([], |row| row.get::<_, String>(1))
        .am("durable request table_info rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("durable request table_info rows for convergence")?;
    drop(table_info);
    // A fresh table's primary key appears as an automatic unique index over one
    // column; the repair creates a named one.
    let mut indexes = conn
        .prepare(
            "SELECT max(ii.name)
             FROM pragma_index_list('durable_requests') AS il
             JOIN pragma_index_info(il.name) AS ii
             WHERE il.\"unique\" = 1 AND il.partial = 0
             GROUP BY il.name
             HAVING count(*) = 1",
        )
        .am("durable request unique index columns for convergence")?;
    // An expression index reports no column name here, so it never counts.
    let unique = indexes
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .am("durable request unique index rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("durable request unique index rows for convergence")?;
    Ok(Some(SqliteShape {
        columns,
        unique_key: unique.iter().flatten().any(|column| column == KEY_COLUMN),
    }))
}

/// The shape facts the Postgres repair decides on: how many of the six request
/// columns the table holds, whether the legacy key is still a column, and whether
/// a unique single-column index backs `request_id`.
struct PgShape {
    /// How many of the six request columns the table holds.
    request_columns: i64,
    /// The legacy key is still a column, whatever its nullability.
    legacy_key: bool,
    /// A unique index over `request_id` alone exists.
    unique_key: bool,
}

impl PgShape {
    /// Whether the table holds all six request columns, no legacy key, and the
    /// unique guarantee, so the repair has nothing to do.
    fn is_current(&self) -> bool {
        self.request_columns == REQUEST_COLUMNS.len() as i64 && !self.legacy_key && self.unique_key
    }
}

/// The `durable_requests` shape, or `None` when the table is absent, which a
/// fresh database relies on: the DDL batch that follows creates the whole shape.
fn pg_shape(client: &mut postgres::Client) -> Result<Option<PgShape>, AmError> {
    let names = request_columns();
    let row = client
        .query_one(
            "SELECT
                 count(*),
                 count(*) FILTER (WHERE column_name = ANY($1)),
                 count(*) FILTER (WHERE column_name = 'id')
             FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'durable_requests'",
            &[&names],
        )
        .am("durable request shape check")?;
    if row.get::<_, i64>(0) == 0 {
        return Ok(None);
    }
    let unique = client
        .query(
            "SELECT a.attname
             FROM pg_index AS i
             JOIN pg_attribute AS a
               ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
             WHERE i.indrelid = to_regclass('durable_requests')
               AND i.indisunique
               AND i.indisvalid
               AND i.indpred IS NULL
               AND i.indexprs IS NULL
               AND i.indnkeyatts = 1",
            &[],
        )
        .am("durable request unique index check")?
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<Vec<_>>();
    Ok(Some(PgShape {
        request_columns: row.get(1),
        legacy_key: row.get::<_, i64>(2) > 0,
        unique_key: unique.iter().any(|column| column == KEY_COLUMN),
    }))
}

/// The name of the `durable_requests` primary key, or `None` when the table has
/// none. The name is read from the catalog because a placeholder may have been
/// created with any key name.
fn pg_primary_key(client: &mut postgres::Client) -> Result<Option<String>, AmError> {
    let rows = client
        .query(
            "SELECT conname FROM pg_constraint
             WHERE conrelid = to_regclass('durable_requests') AND contype = 'p'",
            &[],
        )
        .am("durable request primary key name")?;
    Ok(rows.first().map(|row| row.get::<_, String>(0)))
}

/// Converge a pre-existing Postgres `durable_requests` table onto the request
/// shape.
///
/// The shape is read first, so a database with no table and a table that already
/// holds the converged shape both run no DDL. A table with the legacy `id` key
/// loses that key and its column. Every repaired table gains the request columns
/// it lacks, loses any row that carries no request, and gains the unique index.
/// The statements are idempotent, so a table that still needs one of them can be
/// repaired again.
///
/// # Errors
/// Storage failures, including a value that keeps the unique index from building.
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
                "ALTER TABLE durable_requests DROP CONSTRAINT IF EXISTS \"{}\";",
                name.replace('"', "\"\"")
            ));
        }
        statements.push(format!(
            "ALTER TABLE durable_requests DROP COLUMN IF EXISTS {LEGACY_KEY};"
        ));
    }
    for (column, sql_type) in REQUEST_COLUMNS {
        statements.push(format!(
            "ALTER TABLE durable_requests ADD COLUMN IF NOT EXISTS {column} {sql_type};"
        ));
    }
    // After the columns exist: only a row whose six request columns are all NULL
    // is the leftover placeholder row.
    statements.push(format!(
        "DELETE FROM durable_requests WHERE {};",
        all_columns_null()
    ));
    statements.push(format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS {UNIQUE_INDEX} ON durable_requests ({KEY_COLUMN});"
    ));
    client
        .batch_execute(&statements.join("\n"))
        .am("durable request converge")
}
