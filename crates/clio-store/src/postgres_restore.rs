// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres logical restore: one
//! transaction that clears the target, loads every dumped member back in
//! `COPY` text format, and re-enables what it disabled — so a failure anywhere
//! leaves the target exactly as it was. Every count it reports is measured
//! inside that transaction, before the commit, never the archive's own claim.
//!
//! ## Owns
//! - `restore_postgres`: the one-transaction restore and its observed counts
//! - `PostgresRestoreRequest` / `PostgresRestoreReport`
//! - The pre-flight that refuses before truncating: table ownership, and the
//!   archive's table set against the target's discovered set
//! - `postgres_target_state`: the read-only measurement a restore's blast
//!   radius is built from
//!
//! ## Does not own
//! - The dump transaction (see `postgres_dump`)
//! - The decision to restore, the confirmation, or the pre-restore snapshot
//!   (see `clio`, whose shared restore flow drives both backends)
//! - The key store file swap (see `restore_swap`)
//! - Store open, migration, or embedding-space reconciliation (see `postgres`;
//!   a restore deliberately opens a bare connection instead)
//!
//! ## Boundary
//!
//! Changes to how a restore is placed should normally require changes here.
//! Changes to the archive format or the CLI flow should not require changes
//! here.
//!
//! Keep this module focused on the one-transaction restore.

use crate::migrate::IntoAmError;
use crate::postgres_backup_model::{compare_table_sets, quote_ident};
use crate::postgres_discovery::{DiscoveredSchema, discover_schema};
use crate::postgres_restore_state::{count_tables, read_banks};
use clio_types::{AmError, ErrorCode};
use postgres::Transaction;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Chunk size for streaming a staged member into `COPY ... FROM STDIN`.
const COPY_CHUNK: usize = 64 * 1024;

/// The `am` step name for the restore's own `SELECT COUNT(*)` pass.
///
/// The target's read-only pass names its own step in `postgres_restore_state`.
const RESTORE_COUNT: &str = "count a restored table";

/// One table the restore loads: the member file, its columns, and the rows
/// the archive says it holds. `rows` is the archive's claim, compared against
/// the target's observed count — never a measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreTable {
    /// The table name, unquoted.
    pub table: String,
    /// The archive's column names, in the order the member was written.
    pub columns: Vec<String>,
    /// The staged member file holding the `COPY` text.
    pub member_path: PathBuf,
    /// The rows the archive records for this table.
    pub rows: u64,
}

/// One restore's request: where the target is, which schema it loads into,
/// and every table to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresRestoreRequest {
    /// The target database's URL.
    pub url: String,
    /// The schema the archive was dumped from; the restore loads into it.
    pub schema: String,
    /// Every table to load, in member order.
    pub tables: Vec<RestoreTable>,
}

/// What a finished restore observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresRestoreReport {
    /// The schema the restore loaded into.
    pub schema: String,
    /// Rows the restore transaction was observed to hold, summed over every
    /// counted table — a measurement, never the archive's claim. A
    /// restore that loaded fewer rows than the archive records reports the
    /// smaller figure, and `counts_ok` then reports `false`.
    pub rows_loaded: u64,
    /// Every loaded table with its observed row count, read before the commit.
    pub table_counts: Vec<(String, u64)>,
    /// The distinct bank ids of the restored `items` table, sorted.
    pub banks: Vec<String>,
    /// Whether the observation is clean under the direction rule: no table came
    /// back with fewer rows than the archive holds, and no table the caller's
    /// data lives in came back with more. A surplus in a table the store
    /// rebuilds is the one difference a rebuild absorbs and leaves this `true`.
    pub counts_ok: bool,
    /// Whether this restore's truncate really included a table the store
    /// derives from the content it loads (the vector table today), which is the
    /// only fact that says the derived index is empty afterwards. A restore
    /// whose truncate missed such a table, a dry run, and a backend that keeps
    /// its index inside the store all leave this `false`.
    pub derived_index_emptied: bool,
}

/// Restore one dump into its target, all-or-nothing.
///
/// The order inside one transaction is fixed: discover the target's schema,
/// prove the connected role owns every application table, compare the
/// archive's table set with the target's — every refusal here runs before any
/// truncation — then `TRUNCATE` every base table of the schema in one
/// statement, disable every trigger on the application tables, load each
/// member with an explicit column list, re-enable the triggers, and commit.
/// Any failure rolls the transaction back: the target keeps its original rows,
/// and the trigger disable rolls back with it.
///
/// The `TRUNCATE` covers the schema's base tables beyond the application
/// tables too (the vector table today): its rows are derived data the store
/// rebuilds, and it references `items`, so the single statement must include
/// it or the truncate fails on a populated target.
///
/// The last read before the commit counts every loaded table; that measurement
/// is what the report carries, and the caller reports a mismatch as a failed
/// restore. Counting before the commit is what makes the measurement stable:
/// the transaction still holds the tables, so no other writer can land a row
/// between the load and the count.
///
/// That stability is bought with a wider writer-block window: the count pass
/// runs while the transaction still holds its exclusive locks, so every writer
/// to the target waits for a scan it did not wait for when the measurement came
/// after the commit. The trade is deliberate — a restore is a stop-the-store
/// operation, and an exact count from inside the transaction is the only
/// measurement that can tell a lost row from a concurrent one.
///
/// # Errors
/// The target is unreachable, the schema does not match the target's tables,
/// the connected role does not own the tables, a member is unreadable or its
/// load fails, or the transaction cannot commit.
pub fn restore_postgres(
    request: &PostgresRestoreRequest,
) -> Result<PostgresRestoreReport, AmError> {
    let mut client = crate::pg_tls::connect(&request.url).am("postgres connect for a restore")?;
    let mut tx = client.transaction().am("begin the restore transaction")?;
    // The archive records the schema its tables live in; the restore loads
    // into that schema and points the connection's search_path at it, so any
    // unqualified statement this transaction runs resolves there too.
    tx.batch_execute(&format!(
        "SET search_path TO {}",
        quote_ident(&request.schema)
    ))
    .am("set the restore's search_path")?;
    let discovered = discover_schema(&mut tx, &request.schema)?;
    refuse_non_owning_role(&mut tx, &request.schema)?;
    preflight_table_sets(&discovered, &request.tables)?;
    let derived_index_emptied = truncate(&mut tx, &discovered)?;
    set_triggers(&mut tx, &discovered, "DISABLE")?;
    load_members(&mut tx, &request.schema, &request.tables)?;
    set_triggers(&mut tx, &discovered, "ENABLE")?;
    let table_counts = count_tables(&mut tx, &request.schema, &discovered, RESTORE_COUNT)?;
    let banks = read_banks(&mut tx, &request.schema, &discovered)?;
    tx.commit().am("commit the restore transaction")?;
    Ok(restore_report(
        request,
        table_counts,
        banks,
        derived_index_emptied,
    ))
}

/// Prove the connected role owns every application table.
///
/// `ALTER TABLE ... DISABLE TRIGGER ALL` needs table ownership, which is the
/// normal case — the store's own role owns its tables. A role that cannot own
/// them cannot make the load safe, so this refuses before anything is
/// truncated, with the tables named.
fn refuse_non_owning_role(tx: &mut Transaction, schema: &str) -> Result<(), AmError> {
    let rows = tx
        .query(NON_OWNER_SQL, &[&schema])
        .am("check the connected role's table ownership")?;
    let names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    if names.is_empty() {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::Forbidden,
        format!(
            "the connected role does not own every application table in schema {schema:?} \
             ({names:?}); disabling triggers to load the archive needs table ownership, so \
             restore with a role that owns the store's tables"
        ),
    ))
}

/// The non-owning-table catalog read: base tables of the schema the connected
/// role is not a member of the owner of.
const NON_OWNER_SQL: &str = "
SELECT c.relname
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relkind IN ('r', 'p')
  AND NOT pg_catalog.pg_has_role(current_user, c.relowner, 'MEMBER')
ORDER BY c.relname";

/// Refuse a mismatch between the archive's table set and the target's.
fn preflight_table_sets(
    discovered: &DiscoveredSchema,
    tables: &[RestoreTable],
) -> Result<(), AmError> {
    let target = discovered
        .app_tables
        .iter()
        .map(|table| (table.name.clone(), table.column_names()))
        .collect::<Vec<_>>();
    let archive = tables
        .iter()
        .map(|table| (table.table.clone(), table.columns.clone()))
        .collect::<Vec<_>>();
    compare_table_sets(&target, &archive)
}

/// Clear every base table of the schema in one statement, and report whether
/// that statement included a table the store derives from the content it loads.
///
/// A per-table loop is refused by the foreign keys and can partially empty the
/// target before it fails, so this is one statement or nothing. `RESTART
/// IDENTITY` is harmless today (no owned sequences) and correct if one is
/// ever added with the matching snapshot support.
///
/// The returned fact is the truncate's own: a table counts only when this
/// statement really named it, so a target whose schema carries no derived table
/// reports `false` no matter what its backend is called.
fn truncate(tx: &mut Transaction, discovered: &DiscoveredSchema) -> Result<bool, AmError> {
    let mut names: Vec<String> = discovered
        .app_tables
        .iter()
        .map(|table| table.name.clone())
        .collect();
    names.extend(discovered.extension_dependent.iter().cloned());
    names.sort();
    names.dedup();
    let list = names
        .iter()
        .map(|name| quote_ident(name))
        .collect::<Vec<_>>()
        .join(", ");
    tx.batch_execute(&format!("TRUNCATE TABLE {list} RESTART IDENTITY"))
        .am("truncate the target's tables")?;
    Ok(!discovered.extension_dependent.is_empty())
}

/// Flip every application table's trigger enforcement off or on, always
/// inside the restore transaction.
///
/// The members load in name order, which is not foreign-key order, so the
/// load needs enforcement off; re-enabling it is the last step before the
/// commit, and any failure rolls both directions back with everything else.
fn set_triggers(
    tx: &mut Transaction,
    discovered: &DiscoveredSchema,
    action: &str,
) -> Result<(), AmError> {
    for table in &discovered.app_tables {
        let sql = format!(
            "ALTER TABLE {}.{} {action} TRIGGER ALL",
            quote_ident(&discovered.schema),
            quote_ident(&table.name)
        );
        tx.batch_execute(&sql)
            .map_err(|error| copy_error(action, &table.name, error))?;
    }
    Ok(())
}

/// Load every member with `COPY ... (columns) FROM STDIN`, streaming from its
/// staged file.
///
/// The member order is the archive's, which is name order — not foreign-key
/// order — and that is why the triggers are disabled across the whole load.
/// The column list is the archive's, so a target column the archive does not
/// carry cannot shift the load. The load counts nothing: `COPY`'s own tally is
/// not an observation, so the report's total comes from the counts
/// the restore transaction reads for itself.
fn load_members(
    tx: &mut Transaction,
    schema: &str,
    tables: &[RestoreTable],
) -> Result<(), AmError> {
    for table in tables {
        let columns = table
            .columns
            .iter()
            .map(|column| quote_ident(column))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "COPY {}.{} ({columns}) FROM STDIN",
            quote_ident(schema),
            quote_ident(&table.table)
        );
        let mut source = std::fs::File::open(&table.member_path).map_err(|error| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "reading the staged member for table {:?}: {error}",
                    table.table
                ),
            )
        })?;
        let mut writer = tx
            .copy_in(sql.as_str())
            .map_err(|error| copy_error("starting a load", &table.table, error))?;
        let mut buffer = vec![0u8; COPY_CHUNK];
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|error| copy_error("reading a staged member", &table.table, error))?;
            if read == 0 {
                break;
            }
            writer
                .write_all(&buffer[..read])
                .map_err(|error| copy_error("loading a member", &table.table, error))?;
        }
        writer
            .finish()
            .map_err(|error| copy_error("finishing a load", &table.table, error))?;
    }
    Ok(())
}

/// Build the report from the counts the restore transaction read for itself.
///
/// The counts arrive already measured, from inside the transaction the load
/// ran in, so this assembles them and never reads the database: there is no
/// window between the load and the count for another writer to land a row in.
/// The report's total is the sum of those counts, so `rows_loaded` is what the
/// database was observed to hold and never the archive's own arithmetic.
///
/// `derived_index_emptied` is carried through unchanged: it is what the
/// transaction's truncate reported, and this function never infers it from a
/// count, a backend name, or the shape of the archive.
fn restore_report(
    request: &PostgresRestoreRequest,
    table_counts: Vec<(String, u64)>,
    banks: Vec<String>,
    derived_index_emptied: bool,
) -> PostgresRestoreReport {
    let counts_ok = request.tables.iter().all(|table| {
        let observed = table_counts
            .iter()
            .find(|(name, _)| name == &table.table)
            .map(|(_, count)| *count);
        match observed {
            Some(count) if count == table.rows => true,
            // Over the archive's count only a table the store rebuilds is
            // benign work; every other difference is a row lost or invented.
            Some(count) if count > table.rows => {
                crate::table_data_class(&table.table) == crate::TableDataClass::Derived
            }
            _ => false,
        }
    });
    let rows_loaded = table_counts.iter().map(|(_, count)| count).sum();
    PostgresRestoreReport {
        schema: request.schema.clone(),
        rows_loaded,
        table_counts,
        banks,
        counts_ok,
        derived_index_emptied,
    }
}

/// A `COPY` or trigger-statement failure that names the table and never the
/// stream.
fn copy_error(step: &str, table: &str, error: impl std::fmt::Display) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{step} for table {table:?}: {error}"),
    )
}

/// The report's truncation fact: the rebuild notice's one source of truth.
#[cfg(test)]
#[path = "postgres_restore_index_tests.rs"]
mod index_tests;
/// The restore's count while a second connection waits on its lock.
#[cfg(test)]
#[path = "postgres_restore_race_tests.rs"]
mod race_tests;
/// The restore report's `rows_loaded`: the observation, not the claim.
#[cfg(test)]
#[path = "postgres_restore_rows_tests.rs"]
mod rows_tests;
/// The target measurement's no-store-schema cases.
#[cfg(test)]
#[path = "postgres_restore_state_tests.rs"]
mod state_tests;
#[cfg(test)]
#[path = "postgres_restore_tests.rs"]
mod tests;
