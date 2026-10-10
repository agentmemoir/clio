// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pre-open duplicate-open-triple scan that
//! guards the `triples_open_uniq` unique partial index.
//!
//! ## Owns
//! - Portable duplicate scan SQL (open-edge identity `bank_id + subject + predicate`)
//! - The named `ConfigCorrupt` diagnostic and operator repair text
//!
//! ## Does not own
//! - The unique index DDL itself (`sql/001_core.sql`)
//! - Supersession write ordering (`sqlite_triple` / `postgres_triple`)
//!
//! ## Boundary
//!
//! Changes to when a legacy database may open should normally require changes here.
//! Changes to triple CRUD or query paths should not require changes here.
//!
//! Keep this module focused on the open-time duplicate scan.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};

/// Portable scan: one row per duplicate open-edge identity group.
///
/// `triples` must exist when this runs; callers gate on table presence so a
/// fresh database skips the scan. The COUNT(*) per group is the actionable
/// number; ORDER BY keeps the reported sample deterministic.
pub(crate) const DUPLICATE_SCAN_SQL: &str = "
SELECT bank_id, subject, predicate, count(*) AS n
FROM triples
WHERE valid_until IS NULL AND tx_until IS NULL
GROUP BY bank_id, subject, predicate
HAVING count(*) > 1
ORDER BY n DESC, bank_id, subject, predicate";

/// Named failure for a legacy database that already holds duplicate open
/// edges. Surfaces count plus one sample identity key — never row content.
pub(crate) fn duplicate_open_triples_err(sample: &str, groups: i64) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "duplicate open triples: {groups} (bank_id, subject, predicate) group(s) hold more \
             than one open edge (valid_until IS NULL AND tx_until IS NULL); sample: {sample}; \
             an operator-confirmed repair is required before open: with the store closed, close \
             all but one open edge per subject+predicate (set valid_until and tx_until on the \
             superseded rows — never DELETE), then retry"
        ),
    )
}

/// Build the sample fragment `bank_id=\`b\` subject=\`s\` predicate=\`p\` rows=\`n\``.
fn sample_fragment(bank: &str, subject: &str, predicate: &str, n: i64) -> String {
    format!("bank_id=`{bank}` subject=`{subject}` predicate=`{predicate}` rows={n}")
}

/// True when the `triples` table exists (SQLite database catalog).
pub(crate) fn triples_table_exists_sqlite(conn: &rusqlite::Connection) -> Result<bool, AmError> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'triples'",
            [],
            |r| r.get(0),
        )
        .am("triples table probe")?;
    Ok(present > 0)
}

/// True when `triples` exists on the connected Postgres schema (search_path).
pub(crate) fn triples_table_exists_pg(client: &mut postgres::Client) -> Result<bool, AmError> {
    let present: bool = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = current_schema() AND table_name = 'triples'
             )",
            &[],
        )
        .am("triples table probe pg")?
        .get(0);
    Ok(present)
}

/// Fail store open when the database already holds duplicate open triples.
///
/// Runs before the portable DDL applies `triples_open_uniq`: a raw
/// `CREATE UNIQUE INDEX` build failure must never be the first the operator
/// hears of a legacy duplicate. A fresh database (no `triples` table) skips.
pub(crate) fn ensure_no_duplicate_open_triples_sqlite(
    conn: &rusqlite::Connection,
) -> Result<(), AmError> {
    if !triples_table_exists_sqlite(conn)? {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(DUPLICATE_SCAN_SQL)
        .am("duplicate open triples prepare")?;
    let mut rows = stmt.query([]).am("duplicate open triples query")?;
    let Some(first) = rows.next().am("duplicate open triples row")? else {
        return Ok(());
    };
    let bank: String = first.get(0).am("duplicate open triples bank")?;
    let subject: String = first.get(1).am("duplicate open triples subject")?;
    let predicate: String = first.get(2).am("duplicate open triples predicate")?;
    let n: i64 = first.get(3).am("duplicate open triples count")?;
    let mut groups: i64 = 1;
    while rows.next().am("duplicate open triples row")?.is_some() {
        groups += 1;
    }
    Err(duplicate_open_triples_err(
        &sample_fragment(&bank, &subject, &predicate, n),
        groups,
    ))
}

/// Postgres twin of [`ensure_no_duplicate_open_triples_sqlite`].
pub(crate) fn ensure_no_duplicate_open_triples_pg(
    client: &mut postgres::Client,
) -> Result<(), AmError> {
    if !triples_table_exists_pg(client)? {
        return Ok(());
    }
    let rows = client
        .query(DUPLICATE_SCAN_SQL, &[])
        .am("duplicate open triples scan pg")?;
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let bank: String = first.get(0);
    let subject: String = first.get(1);
    let predicate: String = first.get(2);
    let n: i64 = first.get(3);
    Err(duplicate_open_triples_err(
        &sample_fragment(&bank, &subject, &predicate, n),
        rows.len() as i64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_names_key_and_repair() {
        let err = duplicate_open_triples_err(&sample_fragment("b", "s", "p", 3), 2);
        assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
        let msg = err.to_string();
        assert!(msg.contains("duplicate open triples: 2"), "{msg}");
        assert!(
            msg.contains("bank_id=`b` subject=`s` predicate=`p` rows=3"),
            "{msg}"
        );
        assert!(msg.contains("operator-confirmed"), "{msg}");
    }

    #[test]
    fn sqlite_scan_skips_missing_table_and_flags_duplicates() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory");
        assert!(ensure_no_duplicate_open_triples_sqlite(&conn).is_ok());
        conn.execute_batch(
            "CREATE TABLE triples (
               bank_id text NOT NULL, subject text NOT NULL, predicate text NOT NULL,
               valid_until text, tx_until text)",
        )
        .unwrap();
        assert!(ensure_no_duplicate_open_triples_sqlite(&conn).is_ok());
        conn.execute_batch(
            "INSERT INTO triples VALUES ('b','s','p',NULL,NULL),
                                          ('b','s','p',NULL,NULL),
                                          ('b','s','p','t','t')",
        )
        .unwrap();
        let err = ensure_no_duplicate_open_triples_sqlite(&conn).unwrap_err();
        assert!(err.to_string().contains("rows=2"), "{err}");
    }
}
