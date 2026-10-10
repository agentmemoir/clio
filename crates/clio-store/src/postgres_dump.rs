// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres logical dump: one
//! `REPEATABLE READ READ ONLY` transaction that discovers the application
//! tables and streams every one of them out in `COPY` text format, one staged
//! file per table, digested as it streams.
//!
//! ## Owns
//! - `snapshot_postgres_into`: the one-transaction dump and the facts it reports
//! - `PostgresDumpFacts` / `TableDump`: the schema, extensions, banks, and
//!   per-table member facts the caller records in `dump/meta.json`
//! - The sequence refusal: a schema a logical restore could not reproduce
//! - `PG_METHOD` / `PG_CONSISTENCY`, the manifest values this mechanism gives
//!
//! ## Does not own
//! - Where staged members live (see `clio-backup`, which owns the bytes' landing)
//! - The archive assembly and `dump/meta.json` type (see `clio-backup`)
//! - The restore transaction (see `postgres_restore`)
//! - Store open, migration, or embedding-space reconciliation (see `postgres`;
//!   a snapshot deliberately opens a bare connection instead)
//!
//! ## Boundary
//!
//! Changes to how a dump is taken should normally require changes here.
//! Changes to where the members are staged or how the archive is assembled
//! should not require changes here.
//!
//! Keep this module focused on the one-transaction dump.

use crate::migrate::{IntoAmError, now_iso8601};
use crate::postgres_backup_model::{PgColumn, PgTable, member_name, quote_ident};
use crate::postgres_discovery::{DiscoveredSchema, discover_schema, effective_schema};
use clio_types::{AmError, ErrorCode};
use postgres::Transaction;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::time::Instant;

/// The `snapshot.method` value for the mechanism this module ships.
///
/// Every table is read with `COPY ... TO STDOUT` in Postgres' own text
/// interchange format, so the bytes read back exactly.
pub const PG_METHOD: &str = "postgres-copy";

/// The `snapshot.consistency` value: one read at one instant.
pub const PG_CONSISTENCY: &str = "point-in-time";

/// The transaction statement the whole dump runs inside. Quoted as written:
/// one transaction is what makes the archive a point-in-time snapshot, and
/// `READ ONLY` means a bug in the dump path cannot write.
const SNAPSHOT_TRANSACTION: &str = "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY";

/// Chunk size for streaming a table out and into its staged member.
const COPY_CHUNK: usize = 64 * 1024;

/// One dumped table: the member it became and the facts the dump meta records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDump {
    /// The table name, unquoted.
    pub table: String,
    /// The archive member the rows were streamed into.
    pub member: String,
    /// Bytes of the staged member.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of the staged member.
    pub sha256: String,
    /// Rows copied, counted while streaming.
    pub rows: u64,
    /// The table's ordered columns, as discovered in the same transaction.
    pub columns: Vec<PgColumn>,
}

/// What a finished dump reports back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresDumpFacts {
    /// The effective schema the dump read.
    pub schema: String,
    /// The installed extensions, sorted by name.
    pub extensions: Vec<String>,
    /// The distinct bank ids of the `items` table, sorted; empty when there
    /// is no `items` table or no banks.
    pub banks: Vec<String>,
    /// The store's recorded `schema_version`, when it has one.
    pub schema_version: Option<String>,
    /// Every dumped table, in member order (the discovered, name-sorted order).
    pub tables: Vec<TableDump>,
    /// UTC stamp taken immediately before the transaction began.
    pub started_at: String,
    /// Wall-clock duration of the transaction itself, in milliseconds.
    pub duration_ms: u64,
}

/// The stage sink: one call per discovered table, before its rows stream.
///
/// The caller owns where the bytes land (an owner-only staged file in the
/// backup directory) and hands the open file back. On a dump error the
/// caller removes every path it handed out: the dump closes each handle and
/// never renames anything.
pub type StageTable<'a> =
    dyn FnMut(&PgTable, usize) -> Result<(std::path::PathBuf, std::fs::File), AmError> + 'a;

/// Dump `url`'s store into one staged member per application table.
///
/// The dump opens a bare connection — no migrations, no embedding-space
/// reconciliation, none of the writes a full store open performs — and holds
/// one `REPEATABLE READ READ ONLY` transaction across discovery, every
/// `COPY`, and the banks read. Every step runs on that one snapshot, so the
/// member set and the rows belong to one instant.
///
/// # Errors
/// The database is unreachable, the schema has an owned sequence (a restore
/// could not reproduce its position), the effective schema holds no
/// application tables, a `COPY` or staging step fails, or the transaction
/// cannot commit. On any error the transaction rolls back and every file the
/// caller staged removes itself through the caller's own cleanup.
pub fn snapshot_postgres_into(
    url: &str,
    stage: &mut StageTable<'_>,
) -> Result<PostgresDumpFacts, AmError> {
    snapshot_postgres_with(url, stage, &mut |_, _| Ok(()))
}

/// [`snapshot_postgres_into`] with a probe the tests run inside the snapshot
/// transaction, after discovery and before the copies.
///
/// The probe runs on the dump's own transaction, so a probe that writes is
/// refused by `READ ONLY` — which is one of the assertions the tests make.
/// Production passes a no-op.
pub(crate) fn snapshot_postgres_with(
    url: &str,
    stage: &mut StageTable<'_>,
    probe: &mut dyn FnMut(&mut Transaction, &DiscoveredSchema) -> Result<(), AmError>,
) -> Result<PostgresDumpFacts, AmError> {
    let mut client = crate::pg_tls::connect(url).am("postgres connect for a snapshot")?;
    let began = Instant::now();
    let started_at = now_iso8601();
    let mut tx = client.transaction().am("begin the snapshot transaction")?;
    tx.batch_execute(SNAPSHOT_TRANSACTION)
        .am("set the snapshot transaction read-only repeatable read")?;
    let schema = effective_schema(&mut tx)?;
    let discovered = discover_schema(&mut tx, &schema)?;
    refuse_owned_sequences(&discovered)?;
    refuse_no_app_tables(&discovered)?;
    probe(&mut tx, &discovered)?;
    let tables = dump_tables(&mut tx, &schema, &discovered, stage)?;
    let facts = read_banks_and_version(&mut tx, &schema, &discovered, tables, started_at)?;
    // The commit consumes the transaction: this is the instant the snapshot
    // ends, and every earlier failure left through a rollback instead.
    tx.commit().am("commit the snapshot transaction")?;
    let duration_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(PostgresDumpFacts {
        duration_ms,
        ..facts
    })
}

/// Stream every discovered application table out, one staged member each.
fn dump_tables(
    tx: &mut Transaction,
    schema: &str,
    discovered: &DiscoveredSchema,
    stage: &mut StageTable<'_>,
) -> Result<Vec<TableDump>, AmError> {
    let mut dumps = Vec::with_capacity(discovered.app_tables.len());
    for (index, table) in discovered.app_tables.iter().enumerate() {
        let ordinal = index + 1;
        let member = member_name(ordinal, &table.name)?;
        let (_path, mut file) = stage(table, ordinal)?;
        // The staged file belongs to the caller from here on: this function
        // closes the handle when the table is done and never renames it, and
        // a failure after this point is the caller's cleanup.
        let dump = copy_table(tx, schema, table, &member, &mut file)?;
        dumps.push(dump);
    }
    Ok(dumps)
}

/// Stream one table out in `COPY` text format, digesting and counting as it
/// goes.
///
/// The member is written with the table's discovered column list named
/// explicitly, so the member reads back positionally safely even when the
/// target's table has gained a defaulted column. Rows are counted from the
/// stream itself: `COPY` text format ends every row with one newline, and an
/// embedded newline is escaped rather than raw, so newline bytes equal rows.
fn copy_table(
    tx: &mut Transaction,
    schema: &str,
    table: &PgTable,
    member: &str,
    file: &mut std::fs::File,
) -> Result<TableDump, AmError> {
    let sql = copy_to_stdout_sql(schema, table);
    let mut reader = tx
        .copy_out(sql.as_str())
        .map_err(|error| copy_error("reading a table out", &table.name, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut bytes: u64 = 0;
    let mut rows: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| copy_error("reading a table out", &table.name, error))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| copy_error("writing a staged table member", &table.name, error))?;
        hasher.update(&buffer[..read]);
        bytes += read as u64;
        // One pass over the chunk counts the rows: a `COPY` text row ends
        // with exactly one newline, so newline bytes are rows. The bytecount
        // crate would be a new dependency, which this crate does not take.
        #[allow(clippy::naive_bytecount)]
        let newlines = buffer[..read].iter().filter(|&&byte| byte == b'\n').count() as u64;
        rows += newlines;
    }
    // The writer must finish before the transaction moves on: dropping it
    // without `finish` aborts the copy.
    drop(reader);
    file.sync_all()
        .map_err(|error| copy_error("flushing a staged table member", &table.name, error))?;
    Ok(TableDump {
        table: table.name.clone(),
        member: member.to_owned(),
        bytes,
        sha256: crate::sqlite_snapshot::hex(&hasher.finalize()),
        rows,
        columns: table.columns.clone(),
    })
}

/// `COPY "schema"."table" ("col", ...) TO STDOUT` for one discovered table.
fn copy_to_stdout_sql(schema: &str, table: &PgTable) -> String {
    let columns = table
        .columns
        .iter()
        .map(|column| quote_ident(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "COPY {}.{} ({columns}) TO STDOUT",
        quote_ident(schema),
        quote_ident(&table.name)
    )
}

/// Refuse a schema an owned sequence would make unrestoreable.
fn refuse_owned_sequences(discovered: &DiscoveredSchema) -> Result<(), AmError> {
    let Some(sequence) = discovered.sequences.first() else {
        return Ok(());
    };
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "the schema has an owned sequence: table {:?} column {:?} is supplied by sequence \
             {:?}; a logical restore could not reproduce the sequence's position, so this \
             store cannot be snapshotted until the sequence is removed",
            sequence.table, sequence.column, sequence.sequence
        ),
    ))
}

/// Refuse a schema with nothing to copy.
///
/// A schema with no application tables is either the wrong schema or a target
/// whose store was never opened; dumping it would write an archive that
/// restores into a mismatched target and fails there instead.
fn refuse_no_app_tables(discovered: &DiscoveredSchema) -> Result<(), AmError> {
    if discovered.app_tables.is_empty() {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!(
                "schema {:?} holds no application tables to snapshot; check the connection's \
                 search_path and that the store was opened once",
                discovered.schema
            ),
        ));
    }
    Ok(())
}

/// Read the banks and the schema version, to finish the facts with.
fn read_banks_and_version(
    tx: &mut Transaction,
    schema: &str,
    discovered: &DiscoveredSchema,
    tables: Vec<TableDump>,
    started_at: String,
) -> Result<PostgresDumpFacts, AmError> {
    let banks = if discovered
        .app_tables
        .iter()
        .any(|table| table.name == "items")
    {
        let sql = format!(
            "SELECT DISTINCT bank_id FROM {}.items ORDER BY bank_id",
            quote_ident(schema)
        );
        let rows = tx.query(sql.as_str(), &[]).am("read the store's banks")?;
        rows.iter().map(|row| row.get::<_, String>(0)).collect()
    } else {
        Vec::new()
    };
    let schema_version = read_schema_version(tx, schema, discovered)?;
    Ok(PostgresDumpFacts {
        schema: schema.to_owned(),
        extensions: discovered.extensions.clone(),
        banks,
        schema_version,
        tables,
        started_at,
        duration_ms: 0,
    })
}

/// The store's recorded `schema_version`, when the schema has one.
///
/// A schema without `schema_settings` is not a clio store; the field is
/// optional in the manifest, so this reads `None` rather than refusing.
fn read_schema_version(
    tx: &mut Transaction,
    schema: &str,
    discovered: &DiscoveredSchema,
) -> Result<Option<String>, AmError> {
    if !discovered
        .app_tables
        .iter()
        .any(|table| table.name == "schema_settings")
    {
        return Ok(None);
    }
    let sql = format!(
        "SELECT value FROM {}.schema_settings WHERE key = 'schema_version'",
        quote_ident(schema)
    );
    let row = tx
        .query_opt(sql.as_str(), &[])
        .am("read the schema version")?;
    Ok(row.map(|row| row.get::<_, String>(0)))
}

/// A `COPY` failure that names the table and never the stream.
fn copy_error(step: &str, table: &str, error: impl std::fmt::Display) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{step} for table {table:?}: {error}"),
    )
}

#[cfg(test)]
#[path = "postgres_dump_tests.rs"]
mod tests;

/// The dump's no-store-schema edge.
#[cfg(test)]
#[path = "postgres_dump_edges_tests.rs"]
mod edge_tests;
