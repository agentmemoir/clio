// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the consistent SQLite snapshot: one
//! point-in-time copy of a store file, the facts a backup manifest records
//! about it, and the table counts that prove a copy is complete.
//!
//! ## Owns
//! - `snapshot_sqlite_into`: the read-only source read and the staged copy
//! - `StagedCopy`: removal of a staged copy this run did not finish
//! - `SnapshotFacts` (payload size and digest, the copy's own duration and
//!   start stamp, per-table counts, schema version)
//! - The table enumeration and per-table counting the manifest's completeness
//!   claim rests on
//!
//! ## Does not own
//! - The content digest that survives compaction (see `sqlite_content`)
//! - Where the staged copy lives or what the archive is called (see
//!   `clio-backup`, which owns assembly)
//! - The key-store member (see `snapshot_keys`)
//! - Store open, migration, and bootstrap (see `sqlite`)
//!
//! ## Boundary
//!
//! Changes to how a snapshot is taken or how a copy is checked should
//! normally require changes here. Changes to the archive layout should not.
//!
//! Keep this module focused on the store snapshot and its checks.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode, civil_from_days};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// The `snapshot.method` value for the mechanism this module ships.
///
/// The copy is produced by `VACUUM INTO`, which rebuilds the database: the
/// result is compacted and logically equal to the source, never a byte-for-byte
/// clone. Nothing in the manifest or the CLI may call it a file copy.
pub const METHOD: &str = "vacuum-into";

/// The `snapshot.consistency` value: the copy is one read at one instant.
pub const CONSISTENCY: &str = "point-in-time";

/// The default payload member name for a SQLite store.
pub const PAYLOAD_MEMBER: &str = "store.db";

/// The table that records the store's settings, including its schema version.
const SCHEMA_SETTINGS_TABLE: &str = "schema_settings";

/// Chunk size for streaming digests; a fixed buffer keeps large payloads working.
const READ_CHUNK: usize = 64 * 1024;

/// What a finished snapshot reports back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFacts {
    /// Bytes of the staged copy.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of the staged copy.
    pub sha256: String,
    /// Wall-clock duration of the copy itself, in milliseconds. The digest and
    /// the table counts that follow it are not part of this number.
    pub duration_ms: u64,
    /// UTC `YYYY-MM-DDThh:mm:ssZ` stamp taken immediately before the copy
    /// started.
    pub started_at: String,
    /// Every user table with its row count, ordered by table name. The set is
    /// exhaustive, which is what `counts.complete = true` claims.
    pub table_counts: Vec<(String, u64)>,
    /// The store's recorded `schema_version`, when the store has one.
    pub schema_version: Option<String>,
}

impl SnapshotFacts {
    /// The row count of one table, or `None` when the store has no such table.
    #[must_use]
    pub fn count_of(&self, table: &str) -> Option<u64> {
        self.table_counts
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, count)| *count)
    }

    /// How many tables were counted.
    #[must_use]
    pub fn table_count(&self) -> usize {
        self.table_counts.len()
    }
}

/// Copy `source` to `staged` as one consistent read, then measure the result.
///
/// The source is opened read-only and is never written. `VACUUM INTO` is the
/// mechanism: SQLite reads the whole database as of the start of the vacuum
/// and writes a compacted copy to a new file, so a write that lands during the
/// copy is wholly in the copy or wholly out of it, and the source's journal
/// sidecars are never read or copied. The source is left exactly as it was.
///
/// `staged` must not exist, or must be an empty file: `VACUUM INTO` refuses
/// anything else, and that refusal is what keeps a half-finished file from a
/// previous run out of this one.
///
/// The copy is produced, then digested and counted by reading it and the source
/// once more. The staged file is owner-only (SQLite creates it with the process
/// umask, and the archive half's rule is that a copy of the store is never
/// readable by anyone else), and every failure from the copy onwards removes it
/// through [`StagedCopy`]: SQLite's own warning about an interrupted `VACUUM
/// INTO` producing an incomplete file is exactly why a partial staged copy must
/// not survive. On success the caller owns the file and decides what to do with
/// it; this module never renames it into an archive.
///
/// # Errors
/// A missing or unreadable source, an existing non-empty `staged`, a failed
/// copy, or a digest read failure.
pub fn snapshot_sqlite_into(source: &Path, staged: &Path) -> Result<SnapshotFacts, AmError> {
    if !source.is_file() {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!(
                "no SQLite store at {} to snapshot; check `--db` and the resolved path",
                source.display()
            ),
        ));
    }
    // `backup create` holds the store shared while it copies; a concurrent
    // restore's exclusive hold makes the acquire wait a bounded time and then
    // refuse, so no copy can race the swap.
    let _store_lock = crate::store_lock::SharedHold::acquire(source)?;
    // The store's derived tables are sqlite-vec virtual tables, and a snapshot
    // counts every table, so the extension has to be registered before the
    // counts are read. It is the same registration every store open performs,
    // and it is process-wide, so this is not a second copy of anything.
    crate::vec_register::ensure_sqlite_vec_registered()?;
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .am("open the store read-only for a snapshot")?;
    // The snapshot's own connection counts as an open store handle: the guard
    // holds the registration while the connection is live, so the swap's
    // open-handle count observes this connection too.
    let _handle = crate::open_handles::register(source);
    // Read-only is the connection's open mode, and no `query_only` pragma is
    // set on purpose: `VACUUM INTO` opens a write transaction on the
    // connection even though it never writes the source file, so `query_only`
    // makes the copy fail with "attempt to write a readonly database". The
    // open mode is the stronger guarantee anyway — SQLite cannot write a
    // connection opened `SQLITE_OPEN_READ_ONLY`.
    refuse_existing_target(staged)?;
    // From here the staged path is this run's to clean up: only a returned
    // `Ok` hands it over, so a half-written copy never outlives a failed run.
    let mut staged_copy = StagedCopy::new(staged);
    // The clock and the stamp bracket the copy and nothing else, so the stamp is
    // when the copy began and the duration is the copy's own cost. Both are read
    // before the call because the copy is the part the manifest describes.
    let began = std::time::Instant::now();
    let started_at = now_utc();
    // The INTO argument is a scalar expression, so the target path is bound
    // rather than interpolated: a path with a quote in it cannot become SQL.
    conn.execute("VACUUM INTO ?1", [staged.to_string_lossy().as_ref()])
        .am("copy the store into the staged snapshot")?;
    let duration_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    // SQLite creates the output file itself, with the process umask applied, so
    // the copy is tightened to owner-only here: a staged store is a copy of the
    // operator's memory, and it sits in the backup directory until the archive
    // is published.
    crate::local_kms_migrate::tighten_permissions(staged)
        .am("tighten the staged snapshot permissions")?;

    let facts = measure(staged, &conn, started_at, duration_ms)?;
    drop(conn);
    // The copy is complete and measured, so the caller owns it now.
    staged_copy.release();
    Ok(facts)
}

/// The staged path while this module still owns it.
///
/// SQLite's warning is that an interrupted `VACUUM INTO` can leave an
/// incomplete file behind, so a copy this run did not finish must not outlive
/// the run. One guard over the whole span is what makes that true: a new failure
/// step is covered the moment it is written. A kill is out of its reach.
struct StagedCopy<'a> {
    path: &'a Path,
    owned: bool,
}

impl<'a> StagedCopy<'a> {
    /// Take responsibility for `path` from here on.
    const fn new(path: &'a Path) -> Self {
        Self { path, owned: true }
    }

    /// Hand the finished copy over, because the caller is taking it now.
    const fn release(&mut self) {
        self.owned = false;
    }
}

impl Drop for StagedCopy<'_> {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// Digest and count the staged copy, and attach the span the copy itself took.
///
/// This half only reads the staged file and the source, so removal stays
/// [`StagedCopy`]'s job and a failure here cleans up like one during the copy.
fn measure(
    staged: &Path,
    conn: &Connection,
    started_at: String,
    duration_ms: u64,
) -> Result<SnapshotFacts, AmError> {
    let (bytes, sha256) = digest_file(staged)?;
    let table_counts = count_user_tables(conn)?;
    let schema_version = read_schema_version(conn, &table_counts)?;
    Ok(SnapshotFacts {
        bytes,
        sha256,
        duration_ms,
        started_at,
        table_counts,
        schema_version,
    })
}

/// Refuse a `staged` path that already holds bytes.
fn refuse_existing_target(staged: &Path) -> Result<(), AmError> {
    match std::fs::metadata(staged) {
        Ok(meta) if meta.len() > 0 => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "the staged snapshot path {} already holds {} bytes; a snapshot target must be absent or empty",
                staged.display(),
                meta.len()
            ),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::migrate::map_db_err(
            "reading the staged snapshot path",
            e,
        )),
    }
}

/// Stream `path` once, returning its size and lowercase-hex SHA-256.
fn digest_file(path: &Path) -> Result<(u64, String), AmError> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return Err(crate::migrate::map_db_err(
                "opening the staged snapshot",
                error,
            ));
        }
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut total: u64 = 0;
    loop {
        let read = match file.read(&mut buf) {
            Ok(read) => read,
            Err(error) => {
                return Err(crate::migrate::map_db_err(
                    "reading the staged snapshot",
                    error,
                ));
            }
        };
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        total += read as u64;
    }
    Ok((total, hex(&hasher.finalize())))
}

/// Lowercase-hex encoding of a digest (64 characters for SHA-256).
pub(crate) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// `SELECT COUNT(*)` over one table, with the name quoted so a table whose name
/// contains a quote is still addressed correctly.
fn select_count_from(table: &str) -> String {
    format!("SELECT COUNT(*) FROM \"{}\"", table.replace('"', "\"\""))
}

/// `SELECT *` over one table in `order`, with the name quoted.
pub(crate) fn select_rows_from(table: &str, order: &str) -> String {
    format!(
        "SELECT * FROM \"{}\" ORDER BY {order}",
        table.replace('"', "\"\"")
    )
}

/// Every user table, ordered by name, with its row count.
///
/// Exhaustive on purpose: the manifest claims `counts.complete`, and that claim
/// is only honest when every table was counted. A table the connection cannot
/// read fails the snapshot rather than being silently dropped. `pub(crate)` so
/// the restore's read-only target measurement counts the same way.
pub(crate) fn count_user_tables(conn: &Connection) -> Result<Vec<(String, u64)>, AmError> {
    let mut out = Vec::new();
    for table in user_tables(conn)? {
        let count: i64 = conn
            .query_row(&select_count_from(&table), [], |row| row.get(0))
            .am("count a snapshot table")?;
        out.push((table, u64::try_from(count.max(0)).unwrap_or(0)));
    }
    Ok(out)
}

/// The user tables of the main database, ordered by name.
pub(crate) fn user_tables(conn: &Connection) -> Result<Vec<String>, AmError> {
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .am("list snapshot tables")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .am("list snapshot tables")?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.am("read a snapshot table name")?);
    }
    Ok(out)
}

/// The store's recorded `schema_version`, when it has one.
///
/// A database with no `schema_settings` table is not a clio store, so the
/// version is `None` rather than an error: the manifest's field is optional and
/// the snapshot of the file itself is still valid.
fn read_schema_version(
    conn: &Connection,
    tables: &[(String, u64)],
) -> Result<Option<String>, AmError> {
    if !tables.iter().any(|(name, _)| name == SCHEMA_SETTINGS_TABLE) {
        return Ok(None);
    }
    conn.query_row(
        "SELECT value FROM schema_settings WHERE key = 'schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .am("read the schema version")
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for a Unix second count. Split out from
/// [`now_utc`] so the calendar arithmetic can be tested at known seconds rather
/// than only at whatever second a test happens to run in.
fn utc_stamp(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let rem = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for the current instant (seconds precision).
fn now_utc() -> String {
    utc_stamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
}

#[cfg(test)]
#[path = "sqlite_snapshot_tests.rs"]
mod tests;
