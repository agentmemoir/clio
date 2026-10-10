// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the key store's SQLite schema and format
//! version: the table definitions, the connection pragmas, the format version
//! constant, and the gate that refuses a store this Clio cannot read.
//!
//! ## Owns
//! - `FORMAT_V2`, the one stored format
//! - `init_schema`, `seed_format_version`, `read_format_version`, `check_format_version`
//! - `insert_slot_if_absent`, the one write that creates a slot row
//!
//! ## Does not own
//! - Opening, creating, or migrating a store file (see `local_kms_db`, `local_kms_migrate`)
//! - The v2 stored value shape (see `kms_migrate`)
//! - Key encoding and length checks (see `local_kms_db`)
//!
//! ## Boundary
//!
//! Changes to the tables, pragmas, or the format gate should normally require
//! changes here. A change to what a stored key looks like belongs in
//! `kms_migrate` and must not change the schema. Changes to when a store is
//! opened or migrated should not require changes here.
//!
//! Keep this module focused on the key store's tables and format version.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};

/// The current format: `key_b64` holds base64 sealed under the Clio key.
pub(crate) const FORMAT_V2: &str = "2";

/// Create the schema and set connection pragmas.
///
/// The format version is seeded only when a store is created (see
/// [`seed_format_version`]); an existing store with a missing version fails the
/// format check instead of being silently repaired.
///
/// # Errors
/// The schema cannot be created.
pub(crate) fn init_schema(conn: &Connection) -> Result<(), AmError> {
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = FULL;
         PRAGMA secure_delete = ON;
         PRAGMA busy_timeout = 10000;
         CREATE TABLE IF NOT EXISTS meta (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS slots (
             subject_id TEXT PRIMARY KEY,
             version INTEGER NOT NULL,
             key_b64 TEXT
         ) WITHOUT ROWID;",
    )
    .am("initialize key store schema")
}

/// Seed the current format version when a store is created.
///
/// # Errors
/// The version cannot be written.
pub(crate) fn seed_format_version(conn: &Connection) -> Result<(), AmError> {
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('format_version', ?1)",
        [FORMAT_V2],
    )
    .am("seed key store format version")?;
    Ok(())
}

/// The format version recorded in `meta`, or `None` when the store has none.
///
/// # Errors
/// The version cannot be read.
pub(crate) fn read_format_version(conn: &Connection) -> Result<Option<String>, AmError> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = 'format_version'",
        [],
        |row| row.get(0),
    )
    .optional()
    .am("read key store format version")
}

/// Fail closed when the store was written by a format this Clio cannot read.
///
/// Only the current format is accepted. A v1 store holds plain base64 values
/// under a version this Clio no longer reads, so it is refused rather than read
/// with the wrong assumptions about its values.
///
/// # Errors
/// The version is one this Clio does not read, the store has none, or the
/// version cannot be read.
pub(crate) fn check_format_version(conn: &Connection) -> Result<(), AmError> {
    match read_format_version(conn)?.as_deref() {
        Some(FORMAT_V2) => Ok(()),
        Some(other) => Err(AmError::new(
            ErrorCode::Internal,
            format!("unsupported key store format version {other} (this Clio reads {FORMAT_V2})"),
        )),
        None => Err(AmError::new(
            ErrorCode::Internal,
            "key store is missing its format version",
        )),
    }
}

/// Insert a slot unless the subject already exists.
///
/// The stored value is opaque here: a caller that seals a key passes the sealed
/// text, and a caller that does not passes plaintext. The conflict clause is
/// what makes this safe under two writers.
///
/// # Errors
/// The row cannot be written.
pub(crate) fn insert_slot_if_absent(
    conn: &Connection,
    subject_id: &str,
    version: u32,
    key_b64: &str,
) -> Result<(), AmError> {
    conn.execute(
        "INSERT INTO slots (subject_id, version, key_b64) VALUES (?1, ?2, ?3)
         ON CONFLICT(subject_id) DO NOTHING",
        params![subject_id, version, key_b64],
    )
    .am("insert DEK slot")?;
    Ok(())
}
