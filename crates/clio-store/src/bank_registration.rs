// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module keeps the bank catalog in step with durable bank-scoped rows.
//!
//! ## Owns
//! - The bank-scoped table list used to install insert triggers
//! - SQLite and Postgres trigger setup
//!
//! ## Does not own
//! - Legacy bank backfill, which lives in `sql/001_core.sql`
//! - Bank inventory queries, which live in `ops_store`
//!
//! ## Boundary
//!
//! Add every table with a `bank_id` column to `BANK_SCOPED_TABLES` and the core
//! backfill query when adding a durable bank-scoped table.

use crate::migrate::IntoAmError;
use clio_types::AmError;
use postgres::Client;
use rusqlite::Connection;
use std::fmt::Write as _;

const BANK_SCOPED_TABLES: &[&str] = &[
    "attribute_schemas",
    "continuous_state",
    "continuous_observations",
    "items",
    "index_pending",
    "triples",
    "task_records",
    "failure_records",
    "beliefs",
    "persona_stable_entries",
    "persona_preferences",
    "tasks",
    "failures",
    "memtree_nodes",
    "assoc_edges",
    "audit_events",
    "hygiene_audit_logs",
    "tombstones",
    "compliance_tombstones",
    "sync_cursors",
    "sync_mutations",
    "sync_conflicts",
    "sync_dead_letter",
    "persona_erase_tombstones",
    "distilled_units",
    "distill_intents",
];

pub(crate) fn install_sqlite_triggers(conn: &Connection) -> Result<(), AmError> {
    let mut sql = String::new();
    for table in BANK_SCOPED_TABLES {
        let _ = write!(
            sql,
            "CREATE TRIGGER IF NOT EXISTS clio_register_bank_{table}
             AFTER INSERT ON {table}
             WHEN NEW.bank_id IS NOT NULL AND NEW.bank_id <> ''
             BEGIN
               INSERT INTO banks (id, created_at, updated_at)
               VALUES (NEW.bank_id, '1970-01-01T00:00:00Z', NULL)
               ON CONFLICT (id) DO NOTHING;
             END;\n"
        );
    }
    conn.execute_batch(&sql)
        .am("sqlite bank registration triggers")
}

pub(crate) fn install_postgres_triggers(client: &mut Client) -> Result<(), AmError> {
    let mut sql = String::from(
        "CREATE OR REPLACE FUNCTION clio_register_bank_row()
         RETURNS trigger
         LANGUAGE plpgsql
         AS $$
         BEGIN
           IF NEW.bank_id IS NOT NULL AND NEW.bank_id <> '' THEN
             INSERT INTO banks (id, created_at, updated_at)
             VALUES (NEW.bank_id, '1970-01-01T00:00:00Z', NULL)
             ON CONFLICT (id) DO NOTHING;
           END IF;
           RETURN NEW;
         END;
         $$;\n",
    );
    for table in BANK_SCOPED_TABLES {
        let trigger = format!("clio_register_bank_{table}");
        let _ = write!(
            sql,
            "DROP TRIGGER IF EXISTS {trigger} ON {table};
             CREATE TRIGGER {trigger}
             AFTER INSERT ON {table}
             FOR EACH ROW EXECUTE FUNCTION clio_register_bank_row();\n"
        );
    }
    client
        .batch_execute(&sql)
        .am("postgres bank registration triggers")
}

#[cfg(test)]
#[path = "bank_registration_tests.rs"]
mod tests;
