// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for stamping `schema_settings.schema_version`.
//!
//! ## Owns
//! - The current `schema_version` target constant
//! - Upserting `schema_settings.schema_version` on SQLite and Postgres
//!
//! ## Does not own
//! - Portable DDL text (`sql/001_core.sql`)
//! - Stale-shape detection or wipes
//!
//! ## Boundary
//!
//! Changes to the version target or its stamping should normally require
//! changes here.
//! Changes to item CRUD should not require changes here.
//!
//! Keep this module focused on stamping `schema_settings.schema_version`.

use crate::migrate::{IntoAmError, now_iso};
use clio_types::AmError;

/// Durable schema_settings value after the open-edge unique index addition.
pub(crate) const SCHEMA_VERSION: &str = "1";

/// Force `schema_version` to the current target (upsert).
pub(crate) fn bump_schema_version_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    let ts = now_iso();
    conn.execute(
        "INSERT INTO schema_settings (key, value, created_at, updated_at)
         VALUES ('schema_version', ?1, ?2, ?2)
         ON CONFLICT(key) DO UPDATE SET
           value = excluded.value,
           updated_at = excluded.updated_at",
        rusqlite::params![SCHEMA_VERSION, ts],
    )
    .am("bump schema_version")?;
    Ok(())
}

/// Force `schema_version` to the current target (upsert).
pub(crate) fn bump_schema_version_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    let ts = now_iso();
    let ver = SCHEMA_VERSION.to_owned();
    client
        .execute(
            "INSERT INTO schema_settings (key, value, created_at, updated_at)
             VALUES ('schema_version', $1, $2, $2)
             ON CONFLICT (key) DO UPDATE SET
               value = EXCLUDED.value,
               updated_at = EXCLUDED.updated_at",
            &[&ver, &ts],
        )
        .am("bump schema_version pg")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constant_matches_target() {
        assert_eq!(SCHEMA_VERSION, "1");
    }

    #[test]
    fn sqlite_version_stamp_is_idempotent_upsert() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_settings (
               key text PRIMARY KEY,
               value text NOT NULL,
               created_at text NOT NULL,
               updated_at text
             );",
        )
        .unwrap();
        bump_schema_version_sqlite(&conn).unwrap();
        bump_schema_version_sqlite(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM schema_settings WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }
}
