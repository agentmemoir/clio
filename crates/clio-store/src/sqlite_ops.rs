// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite side of the operator health
//! contract (`OpsStore`): reachability, schema version, bank inventory, and
//! content-free referential-integrity counts.
//!
//! ## Owns
//! - SQLite `OpsStore` SQL (schema_settings, items, triples, assoc_edges, memtree_nodes)
//! - Bank-scoped orphan edge cleanup
//!
//! ## Does not own
//! - The contract definition (`ops_store`)
//! - Finding severity or repair policy (the ops layer)
//!
//! ## Boundary
//!
//! Changes to SQLite operator health SQL should normally require changes here.
//! Changes to the backend-agnostic contract should not require changes here.
//!
//! Keep this module focused on SQLite operator health SQL.

use crate::migrate::IntoAmError;
use crate::ops_store::{IntegritySignals, OpsStore, StoreCounts};
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;

/// COUNT(*) helper: `?1` is the optional bank scope shared by every signal.
fn count(conn: &rusqlite::Connection, sql: &str, bank: Option<&str>) -> Result<u64, AmError> {
    let n: i64 = conn
        .query_row(sql, params![bank], |r| r.get(0))
        .am("ops count")?;
    Ok(u64::try_from(n.max(0)).unwrap_or(0))
}

/// Count a SQLite file's memories and subjects without writing to it.
///
/// The setup switch gate needs the counts before it can decide whether a
/// database change is destructive, and it must leave the file exactly as it
/// found it: the plan's gate test asserts the old store is byte-identical after
/// a successful switch. `SqliteStore::open` runs migrations and updates the
/// file, so this opens `SQLITE_OPEN_READ_ONLY` instead and runs the same two
/// counts the [`OpsStore::counts`] implementation does.
///
/// # Errors
/// The file cannot be opened read-only, or the count query fails (a file that
/// is not a Clio store has no `items` table).
pub fn sqlite_store_counts(path: &Path) -> Result<StoreCounts, AmError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("open read-only: {e}")))?;
    let memories: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .am("counts memories")?;
    let subjects: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT subject_id) FROM items WHERE subject_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .am("counts subjects")?;
    Ok(StoreCounts {
        memories: u64::try_from(memories.max(0)).unwrap_or(0),
        subjects: u64::try_from(subjects.max(0)).unwrap_or(0),
    })
}

impl OpsStore for SqliteStore {
    fn ping(&self) -> Result<(), AmError> {
        self.with_conn(|conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .am("sqlite ping")?;
            Ok(())
        })
    }

    fn schema_version(&self) -> Result<Option<String>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM schema_settings WHERE key = 'schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .am("schema version")
        })
    }

    fn embedding_space(&self) -> Result<crate::embedding_space::EmbeddingSpace, AmError> {
        Ok(self.space().clone())
    }

    fn bank_ids(&self) -> Result<Vec<String>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM banks ORDER BY id")
                .am("bank ids prepare")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .am("bank ids rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("bank id row")?);
            }
            Ok(out)
        })
    }

    fn bank_exists(&self, bank_id: &str) -> Result<bool, AmError> {
        self.with_conn(|conn| {
            let exists = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM banks WHERE id = ?1)",
                    params![bank_id],
                    |row| row.get(0),
                )
                .am("bank exists")?;
            Ok(exists)
        })
    }

    fn register_bank(&self, bank_id: &str, created_at: &str) -> Result<(), AmError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO banks (id, created_at, updated_at) VALUES (?1, ?2, ?2)
                 ON CONFLICT (id) DO NOTHING",
                params![bank_id, created_at],
            )
            .am("bank register")?;
            Ok(())
        })
    }

    fn counts(&self) -> Result<StoreCounts, AmError> {
        self.with_conn(|conn| {
            let memories: i64 = conn
                .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                .am("counts memories")?;
            let subjects: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT subject_id) FROM items WHERE subject_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .am("counts subjects")?;
            Ok(StoreCounts {
                memories: u64::try_from(memories.max(0)).unwrap_or(0),
                subjects: u64::try_from(subjects.max(0)).unwrap_or(0),
            })
        })
    }

    fn integrity_signals(&self, bank: Option<&str>) -> Result<IntegritySignals, AmError> {
        self.with_conn(|conn| {
            let live = "(SELECT id FROM items WHERE (?1 IS NULL OR bank_id = ?1))";
            let orphan_assoc_edges = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM assoc_edges
                     WHERE (?1 IS NULL OR bank_id = ?1)
                       AND (source_id NOT IN {live} OR target_id NOT IN {live})"
                ),
                bank,
            )?;
            let orphan_triple_items = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE (?1 IS NULL OR bank_id = ?1) AND item_id NOT IN {live}"
                ),
                bank,
            )?;
            let dangling_snapshot_refs = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE (?1 IS NULL OR bank_id = ?1)
                       AND snapshot_ref IS NOT NULL AND snapshot_ref NOT IN {live}"
                ),
                bank,
            )?;
            let dangling_gist_refs = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM triples
                     WHERE (?1 IS NULL OR bank_id = ?1)
                       AND gist_ref IS NOT NULL AND gist_ref NOT IN {live}"
                ),
                bank,
            )?;
            let orphan_memtree_leaf_items = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM memtree_nodes
                     WHERE (?1 IS NULL OR bank_id = ?1)
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
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM assoc_edges
                     WHERE bank_id = ?1
                       AND (source_id NOT IN (SELECT id FROM items WHERE bank_id = ?1)
                            OR target_id NOT IN (SELECT id FROM items WHERE bank_id = ?1))",
                    params![bank],
                )
                .am("drop orphan assoc edges")?;
            Ok(n as u64)
        })
    }
}
