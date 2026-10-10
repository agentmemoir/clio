// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `sqlite_integrity_ok`, the store's own
//! structural check of a SQLite database file: the check a verify pass and a
//! restore run on a staged copy before trusting it.
//!
//! ## Owns
//! - `sqlite_integrity_ok`: the read-only open and `PRAGMA integrity_check`
//!   a staged copy must pass
//!
//! ## Does not own
//! - The snapshot copy and its measurement (see `sqlite_snapshot`)
//! - The content digest that survives compaction (see `sqlite_content`)
//! - The store's schema-version stamping and memtree-shape check at open
//!   (see `schema_reshape`, `memtree_schema`)
//!
//! ## Boundary
//!
//! Changes to what makes a copy acceptable should normally require changes
//! here. Changes to how a copy is produced should not.
//!
//! Keep this module focused on the integrity check of a SQLite database file.

use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Ask SQLite whether `path` is a structurally valid database.
///
/// This is the store's own integrity check, called by a verify pass on an
/// extracted payload and by a restore on its staged copy. It opens the file
/// read-only and reads `PRAGMA integrity_check`; a damaged payload names the
/// file it could not read and the rows SQLite could not verify.
///
/// The check is deliberately not part of the store lock: it runs on staged
/// copies, which are never the live store, so taking a hold on them would
/// only leave lock files beside files no restore can be running against.
///
/// # Errors
/// The file is missing, cannot be opened as a database, or fails the check.
pub fn sqlite_integrity_ok(path: &Path) -> Result<(), AmError> {
    if !path.is_file() {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!("no payload at {} to check", path.display()),
        ));
    }
    let damaged = |detail: String| {
        AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "the payload at {} is not a valid SQLite database: {detail}",
                path.display()
            ),
        )
    };
    let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(error) => return Err(damaged(format!("it cannot be opened: {error}"))),
    };
    let verdict: String = match conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)) {
        Ok(verdict) => verdict,
        Err(error) => {
            return Err(damaged(format!(
                "the integrity check could not run: {error}"
            )));
        }
    };
    if verdict != "ok" {
        return Err(damaged(verdict));
    }
    Ok(())
}
