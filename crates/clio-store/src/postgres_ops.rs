// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres side of the operator health
//! contract (`OpsStore`): reachability, schema version, bank inventory, and
//! content-free referential-integrity counts.
//!
//! ## Owns
//! - Postgres `OpsStore` SQL (schema_settings, items, triples, assoc_edges, memtree_nodes)
//! - Bank-scoped orphan edge cleanup
//!
//! ## Does not own
//! - The contract definition (`ops_store`)
//! - Finding severity or repair policy (the ops layer)
//!
//! ## Boundary
//!
//! Changes to Postgres operator health SQL should normally require changes here.
//! Changes to the backend-agnostic contract should not require changes here.
//!
//! Keep this module focused on Postgres operator health SQL.

use crate::migrate::IntoAmError;
use crate::ops_store::{IntegritySignals, OpsStore, StoreCounts};
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};
use postgres::Client;

/// COUNT(*) helper: `$1` is the optional bank scope shared by every signal.
fn count(client: &mut Client, sql: &str, bank: Option<&str>) -> Result<u64, AmError> {
    let row = client.query_one(sql, &[&bank]).am("ops count")?;
    let n: i64 = row.get(0);
    Ok(u64::try_from(n.max(0)).unwrap_or(0))
}

impl OpsStore for PostgresStore {
    fn ping(&self) -> Result<(), AmError> {
        self.with_client(|client| {
            client.query_one("SELECT 1", &[]).am("postgres ping")?;
            Ok(())
        })
    }

    fn schema_version(&self) -> Result<Option<String>, AmError> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    "SELECT value FROM schema_settings WHERE key = 'schema_version'",
                    &[],
                )
                .am("schema version")?;
            Ok(row.map(|r| r.get::<_, String>(0)))
        })
    }

    fn embedding_space(&self) -> Result<crate::embedding_space::EmbeddingSpace, AmError> {
        Ok(self.space().clone())
    }

    fn bank_ids(&self) -> Result<Vec<String>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query("SELECT DISTINCT bank_id FROM items ORDER BY bank_id", &[])
                .am("bank ids")?;
            Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
        })
    }

    fn counts(&self) -> Result<StoreCounts, AmError> {
        self.with_client(|client| {
            let memories: i64 = client
                .query_one("SELECT COUNT(*) FROM items", &[])
                .am("counts memories")?
                .get(0);
            let subjects: i64 = client
                .query_one(
                    "SELECT COUNT(DISTINCT subject_id) FROM items WHERE subject_id IS NOT NULL",
                    &[],
                )
                .am("counts subjects")?
                .get(0);
            Ok(StoreCounts {
                memories: u64::try_from(memories.max(0)).unwrap_or(0),
                subjects: u64::try_from(subjects.max(0)).unwrap_or(0),
            })
        })
    }

    fn integrity_signals(&self, bank: Option<&str>) -> Result<IntegritySignals, AmError> {
        self.with_client(|client| {
            let live = "(SELECT id FROM items WHERE ($1::text IS NULL OR bank_id = $1))";
            let orphan_assoc_edges = count(
                client,
                &format!(
                    "SELECT COUNT(*) FROM assoc_edges
                     WHERE ($1::text IS NULL OR bank_id = $1)
                       AND (source_id NOT IN {live} OR target_id NOT IN {live})"
                ),
                bank,
            )?;
            let orphan_triple_items = count(
                client,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE ($1::text IS NULL OR bank_id = $1) AND item_id NOT IN {live}"
                ),
                bank,
            )?;
            let dangling_snapshot_refs = count(
                client,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE ($1::text IS NULL OR bank_id = $1)
                       AND snapshot_ref IS NOT NULL AND snapshot_ref NOT IN {live}"
                ),
                bank,
            )?;
            let dangling_gist_refs = count(
                client,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE ($1::text IS NULL OR bank_id = $1)
                       AND gist_ref IS NOT NULL AND gist_ref NOT IN {live}"
                ),
                bank,
            )?;
            let orphan_memtree_leaf_items = count(
                client,
                &format!(
                    "SELECT COUNT(*) FROM memtree_nodes
                     WHERE ($1::text IS NULL OR bank_id = $1)
                       AND item_id IS NOT NULL AND item_id NOT IN {live}"
                ),
                bank,
            )?;
            Ok(IntegritySignals {
                orphan_assoc_edges,
                orphan_triple_items,
                dangling_snapshot_refs,
                dangling_gist_refs,
                orphan_memtree_leaf_items,
            })
        })
    }

    fn drop_orphan_assoc_edges(&self, bank: &str) -> Result<u64, AmError> {
        if bank.trim().is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        self.with_client(|client| {
            let n = client
                .execute(
                    "DELETE FROM assoc_edges
                     WHERE bank_id = $1
                       AND (source_id NOT IN (SELECT id FROM items WHERE bank_id = $1)
                            OR target_id NOT IN (SELECT id FROM items WHERE bank_id = $1))",
                    &[&bank],
                )
                .am("drop orphan assoc edges")?;
            Ok(n)
        })
    }
}

#[cfg(test)]
#[path = "postgres_ops_tests.rs"]
mod postgres_ops_tests;
