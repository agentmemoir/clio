// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for refusing to open a store whose `triples`
//! table still carries the legacy plaintext `object` column. The runtime
//! reshape that sealed such values was removed; only the open guard remains.
//!
//! ## Owns
//! - Detecting the legacy plaintext shape on SQLite and Postgres
//! - The stable `ConfigCorrupt` refusal both backends surface on open
//!
//! ## Does not own
//! - The sealed value's seal/open contract (see `triple_cipher`)
//! - Fresh-database column declaration (the portable core DDL)
//! - Additive, value-free column convergence (see `schema_converge`)
//! - Duplicate-open-triple scanning (see `triple_invariant`)
//!
//! ## Boundary
//!
//! Changes to when a legacy-shaped store may open should normally require
//! changes here. Changes to the sealed value's runtime contract should not
//! require changes here.
//!
//! Keep this module focused on the legacy-shape open guard.

use crate::migrate::IntoAmError;
use crate::triple_invariant::{triples_table_exists_pg, triples_table_exists_sqlite};
use clio_types::{AmError, ErrorCode};

/// The stable refusal for a legacy-shaped `triples` table.
///
/// The runtime reshape that sealed legacy plaintext values no longer exists, so
/// an operator sees this same message on every open until the store was
/// converted by a release that still carried the reshape.
fn legacy_shape_error() -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        "refusing to open: the triples table still has the legacy plaintext \
         `object` column, which this build no longer seals or converts; convert \
         the store with a release that still carries the sealed-value reshape, \
         or restore a converted backup, then retry",
    )
}

/// True when the SQLite `triples` table exists with the plaintext `object`
/// column. A fresh database has no `triples` table and is not stale.
pub(crate) fn sqlite_triples_stale(conn: &rusqlite::Connection) -> Result<bool, AmError> {
    if !triples_table_exists_sqlite(conn)? {
        return Ok(false);
    }
    let mut stmt = conn
        .prepare("PRAGMA table_info(triples)")
        .am("pragma table_info triples")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .am("triples table_info rows")?;
    for name in names {
        if name.am("triples table_info name")? == "object" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True when the connected Postgres schema's `triples` table exists with the
/// plaintext `object` column.
pub(crate) fn pg_triples_stale(client: &mut postgres::Client) -> Result<bool, AmError> {
    if !triples_table_exists_pg(client)? {
        return Ok(false);
    }
    client
        .query_one(
            "SELECT EXISTS (
                 SELECT 1 FROM information_schema.columns
                 WHERE table_schema = current_schema() AND table_name = 'triples'
                   AND column_name = 'object'
             )",
            &[],
        )
        .am("pg triples object col")
        .map(|r| r.get(0))
}

/// Fail store open when the SQLite `triples` table still has the legacy
/// plaintext `object` column. Callers run this before anything writes, so a
/// refused open leaves the database unchanged.
///
/// # Errors
/// The stable [`legacy_shape_error`] on a legacy-shaped table; probe failures.
pub(crate) fn refuse_legacy_triples_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    if sqlite_triples_stale(conn)? {
        return Err(legacy_shape_error());
    }
    Ok(())
}

/// Postgres twin of [`refuse_legacy_triples_sqlite`].
///
/// # Errors
/// The stable [`legacy_shape_error`] on a legacy-shaped table; probe failures.
pub(crate) fn refuse_legacy_triples_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    if pg_triples_stale(client)? {
        return Err(legacy_shape_error());
    }
    Ok(())
}

#[cfg(test)]
#[path = "triple_reshape_tests.rs"]
mod tests;
