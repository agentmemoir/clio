// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for adding the `sync_device_high_water` table to
//! a database that was bootstrapped before the table existed, on both backends.
//!
//! ## Owns
//! - The idempotent `sync_device_high_water` DDL an existing database runs
//!
//! ## Does not own
//! - The fresh-database declaration the migrate batch embeds
//! - The `SyncStore` contract or its SQL (`sync_store`, `sync_state`)
//! - Other slot-table convergence (`local_events_converge`,
//!   `durable_requests_converge`, `copy_inventory_converge`,
//!   `destination_custody_converge`)
//!
//! ## Boundary
//!
//! Changes to which tables a pre-existing database gains should normally require
//! changes here. Changes to the fresh-database declaration should not.
//!
//! Keep this module focused on the sync table a bootstrapped database lacks.
//!
//! # Why this step exists
//!
//! Postgres stamps `postgres_bootstrap_version` once the whole bootstrap batch
//! has run, and a later open that finds the stamp current skips that batch
//! entirely. SQLite re-runs the portable batch on every open, so it already
//! gains a new table there. Postgres does not, and a database bootstrapped
//! before `sync_device_high_water` existed never runs the batch again, so the
//! table has to arrive the way the sibling slot tables arrive: a convergence
//! pass that runs on every open, before the batch, and is a no-op once the shape
//! is current.
//!
//! A plain `CREATE TABLE IF NOT EXISTS` is enough for this table. An earlier
//! shape carried `PRIMARY KEY (bank_id, device_id)`, but only on this branch
//! before it merged, so no shipped database holds it. A database that did get
//! that shape keeps it, because `IF NOT EXISTS` leaves an existing table alone;
//! dropping it once is a local development cleanup, not a migration step.

use crate::migrate::IntoAmError;
use clio_types::AmError;

/// The table, as a fresh database declares it.
///
/// This is deliberately a second statement of the table rather than a reference
/// to the migrate batch: an already-bootstrapped database never runs that batch
/// again, so the convergence step must carry the shape itself. It must stay
/// identical to the fresh declaration the batch embeds; a column added there and
/// not here leaves an existing database short, and the convergence tests on both
/// backends compare the two.
///
/// `PRIMARY KEY (bank_id, device_id, entity_kind)` is the only index the table
/// needs: every read filters on all three columns, and every erasure filters on
/// the leading one.
const SYNC_DEVICE_HIGH_WATER_DDL: &str = "
CREATE TABLE IF NOT EXISTS sync_device_high_water
(
  bank_id          text NOT NULL,
  device_id        text NOT NULL,
  entity_kind      text NOT NULL,
  transaction_time text NOT NULL,
  updated_at       text NOT NULL,
  PRIMARY KEY (bank_id, device_id, entity_kind)
);
";

/// Create `sync_device_high_water` on an existing SQLite database.
///
/// A fresh database is untouched: the portable DDL declares the table itself.
/// The statement is `IF NOT EXISTS`, so a second open is a no-op.
///
/// # Errors
/// Storage failures.
pub(crate) fn converge_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    conn.execute_batch(SYNC_DEVICE_HIGH_WATER_DDL)
        .am("sync device high water converge")
}

/// Create `sync_device_high_water` on an existing Postgres database.
///
/// A fresh database is untouched: the migrate batch declares the table itself,
/// and this statement is a no-op by then. On a database whose bootstrap is
/// already current this is the only statement that creates the table at all.
///
/// # Errors
/// Storage failures.
pub(crate) fn converge_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    client
        .batch_execute(SYNC_DEVICE_HIGH_WATER_DDL)
        .am("sync device high water converge")
}

#[cfg(test)]
#[path = "sync_schema_converge_tests.rs"]
mod sync_schema_converge_tests;

#[cfg(test)]
#[path = "sync_schema_converge_pg_tests.rs"]
mod sync_schema_converge_pg_tests;
