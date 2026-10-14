// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converging a pre-existing single-key
//! placeholder `copy_inventory` table onto the metadata columns the store
//! writes, on both backends, without running DDL against a table that already
//! has the full shape and a unique guarantee on `copy_id`.
//!
//! ## Owns
//! - The legacy-shape detection for both backends
//! - The additive metadata-column repair and the legacy `id` key repair
//! - The unique guarantee on `copy_id` the save's conflict target needs: its
//!   detection and the unique index that provides it
//!
//! ## Does not own
//! - The `copy_inventory` slot DDL text (loaded through `migrate`)
//! - The save/load contract and the row shape (`copy_inventory`)
//! - The store connection helpers (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how a legacy `copy_inventory` table is repaired should normally
//! require changes here. Changes to which columns the store writes should not
//! require changes here.
//!
//! Keep this module focused on placeholder convergence.

use crate::migrate::IntoAmError;
use clio_types::AmError;

/// The columns a pre-existing single-key placeholder table converges onto,
/// in `PRAGMA table_info` check order.
const SQLITE_CONVERGE_COLUMNS: [&str; 8] = [
    "copy_id",
    "request_id",
    "custody",
    "owner",
    "required_action",
    "state",
    "exception_reason",
    "updated_at",
];

/// Whether SQLite already backs the save's conflict target with a unique
/// guarantee on `copy_id`: a non-partial unique index whose key is exactly
/// `copy_id`. The autoindex of a fresh table's `copy_id` primary key counts;
/// a partial index and a wider index do not, because `ON CONFLICT (copy_id)`
/// infers no constraint from either.
fn sqlite_copy_id_is_unique(conn: &rusqlite::Connection) -> Result<bool, AmError> {
    let mut list = conn
        .prepare("PRAGMA index_list(copy_inventory)")
        .am("copy inventory index list for convergence")?;
    let indexes = list
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .am("copy inventory index list rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("copy inventory index list rows for convergence")?;
    drop(list);
    for (name, unique, partial) in indexes {
        if unique == 0 || partial != 0 {
            continue;
        }
        let mut info = conn
            .prepare(&format!("PRAGMA index_info({})", name.replace('\'', "''")))
            .am("copy inventory index info for convergence")?;
        let key = info
            .query_map([], |r| r.get::<_, Option<String>>(2))
            .am("copy inventory index info rows for convergence")?
            .collect::<Result<Vec<_>, _>>()
            .am("copy inventory index info rows for convergence")?;
        drop(info);
        if key.len() == 1 && key[0].as_deref() == Some("copy_id") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Add the metadata columns to a pre-existing single-key placeholder
/// `copy_inventory` table, and the unique index over `copy_id` the save's
/// conflict target needs. A table that already has the full shape and that
/// unique guarantee — a fresh database, whose slot DDL declares `copy_id` as
/// its primary key, or an already-repaired table — is left untouched, so the
/// save and read paths run no DDL; a full-shape table whose guarantee is
/// missing, which an interrupted convergence leaves behind, is healed here on
/// the next use. SQLite has no `ADD COLUMN IF NOT EXISTS`, so each column is
/// checked against `PRAGMA table_info` first. The added columns stay nullable
/// because SQLite only accepts a `NOT NULL` added column with a constant
/// default: the store always writes every field, so a row written after this
/// holds real values. The original `id` key stays in place, so the repair also
/// adds a unique index over `copy_id`: without it the save's conflict target
/// matches no constraint on a repaired table. Placeholder tables hold no copy
/// rows by contract, so converged rows start empty and fill on save.
pub(crate) fn converge_copy_inventory_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'copy_inventory'",
            [],
            |r| r.get(0),
        )
        .am("copy inventory table exists for convergence")?;
    if exists == 0 {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("PRAGMA table_info(copy_inventory)")
        .am("copy inventory table_info for convergence")?;
    let present = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .am("copy inventory table_info rows for convergence")?
        .collect::<Result<Vec<_>, _>>()
        .am("copy inventory table_info rows for convergence")?;
    drop(stmt);
    let full_shape = SQLITE_CONVERGE_COLUMNS
        .iter()
        .all(|column| present.iter().any(|name| name == column));
    if full_shape && sqlite_copy_id_is_unique(conn)? {
        return Ok(());
    }
    for column in SQLITE_CONVERGE_COLUMNS {
        if !present.iter().any(|name| name == column) {
            conn.execute(
                &format!("ALTER TABLE copy_inventory ADD COLUMN {column} text"),
                [],
            )
            .am("add converged copy inventory column")?;
        }
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS copy_inventory_copy_id_uidx
         ON copy_inventory (copy_id)",
        [],
    )
    .am("copy inventory copy-id index")?;
    Ok(())
}

/// The shape facts the Postgres convergence decision needs: how many of the
/// eight metadata columns `copy_inventory` has, and whether it still carries
/// the legacy `id` primary key, which Postgres makes NOT NULL.
fn pg_shape(client: &mut postgres::Client) -> Result<(i64, bool), AmError> {
    let row = client
        .query_one(
            "SELECT
                 count(*) FILTER (WHERE column_name IN (
                     'copy_id', 'request_id', 'custody', 'owner',
                     'required_action', 'state', 'exception_reason', 'updated_at'
                 )),
                 count(*) FILTER (WHERE column_name = 'id' AND is_nullable = 'NO')
             FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'copy_inventory'",
            &[],
        )
        .am("copy inventory shape check")?;
    Ok((row.get::<_, i64>(0), row.get::<_, i64>(1) > 0))
}

/// Whether Postgres already backs the save's conflict target with a unique
/// guarantee on `copy_id`: a valid, non-partial unique index whose key is
/// exactly `copy_id`. The primary-key index of a fresh table counts; a
/// partial index does not, because `ON CONFLICT (copy_id)` infers no arbiter
/// from one.
fn pg_copy_id_is_unique(client: &mut postgres::Client) -> Result<bool, AmError> {
    let backed: bool = client
        .query_one(
            "SELECT EXISTS (
                 SELECT 1
                 FROM pg_index i
                 JOIN pg_class t ON t.oid = i.indrelid
                 JOIN pg_namespace n ON n.oid = t.relnamespace
                 WHERE n.nspname = current_schema()
                   AND t.relname = 'copy_inventory'
                   AND i.indisunique AND i.indisvalid
                   AND i.indpred IS NULL
                   AND i.indnatts = 1
                   AND i.indkey[0] = (
                       SELECT a.attnum FROM pg_attribute a
                       WHERE a.attrelid = t.oid
                         AND a.attname = 'copy_id'
                         AND NOT a.attisdropped
                   )
             )",
            &[],
        )
        .am("copy inventory unique index check")?
        .get(0);
    Ok(backed)
}

/// Bring a pre-existing single-key placeholder `copy_inventory` table onto
/// the metadata columns. The shape and the unique guarantee on `copy_id` are
/// checked first, so a table that already has both runs no DDL on the save or
/// read path; a full-shape table whose unique index is missing — an
/// interrupted repair — is healed by the repair below. A legacy table also
/// carries `id text PRIMARY KEY`, which Postgres makes NOT NULL: the repair
/// drops that key and its NOT NULL, keeping the column optional so no
/// placeholder value is destroyed, and backs the save's conflict target with
/// a unique index over `copy_id`. The slot DDL is portable across both
/// backends, so Postgres cannot carry these repairs in the shared slot file
/// the way the vector file does.
pub(crate) fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    let (metadata_columns, legacy_key) = pg_shape(client)?;
    if metadata_columns == 8 && !legacy_key && pg_copy_id_is_unique(client)? {
        return Ok(());
    }
    if legacy_key {
        client
            .batch_execute(
                "ALTER TABLE copy_inventory DROP CONSTRAINT IF EXISTS copy_inventory_pkey;
                 ALTER TABLE copy_inventory ALTER COLUMN id DROP NOT NULL;",
            )
            .am("copy inventory legacy key repair")?;
    }
    client
        .batch_execute(
            "ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS copy_id text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS request_id text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS custody text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS owner text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS required_action text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS state text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS exception_reason text;
             ALTER TABLE copy_inventory ADD COLUMN IF NOT EXISTS updated_at text;
             CREATE UNIQUE INDEX IF NOT EXISTS copy_inventory_copy_id_uidx
                 ON copy_inventory (copy_id);",
        )
        .am("copy inventory converge")
}
