// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only measurement of a Postgres
//! store target: the effective schema, its application tables with their
//! columns, their row counts, and the banks.
//!
//! ## Owns
//! - `PostgresTargetState` and the queries that count a table and read a bank
//! - `postgres_target_state`: the one bare-connection measurement every
//!   Postgres restore and dry run builds its plan from
//!
//! ## Does not own
//! - The one-transaction restore that loads a dump (see `postgres_restore`)
//! - Catalog discovery itself (see `postgres_discovery`)
//! - Identifier quoting (see `postgres_backup_model`)
//!
//! ## Boundary
//!
//! Changes to what a target measurement reads should normally require changes
//! here. Changes to how a restore loads rows should not require changes here.
//!
//! Keep this module focused on the read-only target measurement.

use crate::postgres_backup_model::{PgTable, quote_ident};
use crate::postgres_discovery::{DiscoveredSchema, discover_schema, effective_schema};
/// The failure text a target-measurement count carries.
const TARGET_COUNT: &str = "count a target table";
use crate::migrate::IntoAmError;
use clio_types::AmError;
use postgres::Transaction;

/// The measurement opens a bare connection — no migrations, no writes — and
/// reads the effective schema, its application tables with their columns and
/// row counts, and the banks. A restore's blast radius, its table-set
/// pre-flight, and its pre-restore decision are all built from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresTargetState {
    /// The effective schema the measurement read.
    pub schema: String,
    /// The application tables with their ordered columns, sorted by name.
    pub tables: Vec<PgTable>,
    /// Every application table with its row count, sorted by table name.
    pub table_counts: Vec<(String, u64)>,
    /// The distinct bank ids of the `items` table, sorted.
    pub banks: Vec<String>,
}

impl PostgresTargetState {
    /// The row count of one table, or `None` when there is no such table.
    #[must_use]
    pub fn count_of(&self, table: &str) -> Option<u64> {
        self.table_counts
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, count)| *count)
    }

    /// How many application tables were counted.
    #[must_use]
    pub fn table_count(&self) -> usize {
        self.table_counts.len()
    }

    /// Whether the store holds no memory rows: no `items` and no `triples`.
    /// The same rule the SQLite target measurement applies, so the pre-restore
    /// decision cannot differ between backends.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count_of("items").unwrap_or(0) == 0 && self.count_of("triples").unwrap_or(0) == 0
    }
}

/// Measure a Postgres store target read-only.
///
/// # Errors
/// The database is unreachable, or a catalog or count read fails.
pub fn postgres_target_state(url: &str) -> Result<PostgresTargetState, AmError> {
    let mut client = crate::pg_tls::connect(url).am("postgres connect to measure a target")?;
    let mut tx = client.transaction().am("begin the target measurement")?;
    let schema = effective_schema(&mut tx)?;
    let discovered = discover_schema(&mut tx, &schema)?;
    let table_counts = count_tables(&mut tx, &schema, &discovered, TARGET_COUNT)?;
    let banks = read_banks(&mut tx, &schema, &discovered)?;
    tx.commit().am("commit the target measurement")?;
    Ok(PostgresTargetState {
        schema,
        tables: discovered.app_tables.clone(),
        table_counts,
        banks,
    })
}

/// One `SELECT COUNT(*)` per application table, in discovered (name) order.
///
/// Both measurements on this module count the same way: the restore's
/// post-commit counts and the target's pre-restore counts, so a mismatch
/// between the two is never an artifact of how either was counted.
pub(crate) fn count_tables(
    tx: &mut Transaction,
    schema: &str,
    discovered: &DiscoveredSchema,
    step: &'static str,
) -> Result<Vec<(String, u64)>, AmError> {
    let mut counts = Vec::with_capacity(discovered.app_tables.len());
    for table in &discovered.app_tables {
        let sql = format!(
            "SELECT COUNT(*) FROM {}.{}",
            quote_ident(schema),
            quote_ident(&table.name)
        );
        let count: i64 = tx.query_one(sql.as_str(), &[]).am(step)?.get(0);
        counts.push((table.name.clone(), u64::try_from(count.max(0)).unwrap_or(0)));
    }
    Ok(counts)
}

/// The store's bank ids, sorted, for a report or a pre-restore decision.
///
/// A target with no `items` table has no banks, which is a fact rather than an
/// error: the table may simply not exist in this schema.
pub(crate) fn read_banks(
    tx: &mut Transaction,
    schema: &str,
    discovered: &DiscoveredSchema,
) -> Result<Vec<String>, AmError> {
    if !discovered
        .app_tables
        .iter()
        .any(|table| table.name == "items")
    {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT DISTINCT bank_id FROM {}.items ORDER BY bank_id",
        quote_ident(schema)
    );
    let rows = tx.query(sql.as_str(), &[]).am("read the store's banks")?;
    Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
}
