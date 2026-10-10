// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite index-pipeline coverage counting.
//!
//! ## Owns
//! - `index_coverage_sqlite` (indexable / dense / lexical / queue counts)
//!
//! ## Does not own
//! - Lexical docs, search, or queue SQL (`sqlite_index`)
//! - The backend-agnostic contract (`index_store`)
//!
//! ## Boundary
//!
//! Changes to SQLite coverage SQL should normally require changes here.
//! Changes to search or queue behavior should not require changes here.
//!
//! Keep this module focused on SQLite coverage SQL.

use crate::migrate::IntoAmError;
use crate::model::IndexCoverage;
use clio_types::AmError;
use rusqlite::{Connection, OptionalExtension, params};

/// Coverage counts over the shared `items` columns (portable SQL).
pub(crate) fn index_coverage_sqlite(
    conn: &Connection,
    bank_id: Option<&str>,
) -> Result<IndexCoverage, AmError> {
    let bank_where = match bank_id {
        Some(_) => " AND items.bank_id = ?1",
        None => "",
    };
    let count = |sql: &str| -> Result<i64, AmError> {
        if bank_id.is_some() {
            conn.query_row(sql, params![bank_id], |r| r.get(0))
                .am("coverage count")
        } else {
            conn.query_row(sql, [], |r| r.get(0)).am("coverage count")
        }
    };
    let indexable = count(&format!(
        "SELECT COUNT(*) FROM items
         WHERE items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
    ))?;
    let dense = count(&format!(
        "SELECT COUNT(*) FROM item_embeddings e
         JOIN items ON items.id = e.item_id AND items.bank_id = e.bank_id
         WHERE items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
    ))?;
    let lexical = count(&format!(
        "SELECT COUNT(*) FROM items_fts
         JOIN items ON items.id = items_fts.item_id AND items.bank_id = items_fts.bank_id
         WHERE items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
    ))?;
    let last_error: Option<String> = if bank_id.is_some() {
        conn.query_row(
            "SELECT last_error FROM index_pending
             WHERE bank_id = ?1 AND last_error IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            params![bank_id],
            |r| r.get(0),
        )
        .optional()
        .am("coverage last_error")?
    } else {
        conn.query_row(
            "SELECT last_error FROM index_pending WHERE last_error IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .am("coverage last_error")?
    };
    let (pending, retry): (i64, i64) = if bank_id.is_some() {
        conn.query_row(
            "SELECT COUNT(*), COUNT(*) FILTER (WHERE attempts > 0) FROM index_pending
             WHERE bank_id = ?1",
            params![bank_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .am("coverage queue")?
    } else {
        conn.query_row(
            "SELECT COUNT(*), COUNT(*) FILTER (WHERE attempts > 0) FROM index_pending",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .am("coverage queue")?
    };
    Ok(IndexCoverage {
        bank_id: bank_id.map(str::to_owned),
        indexable_items: u64::try_from(indexable.max(0)).unwrap_or(0),
        dense_rows: u64::try_from(dense.max(0)).unwrap_or(0),
        lexical_rows: u64::try_from(lexical.max(0)).unwrap_or(0),
        pending_jobs: u64::try_from(pending.max(0)).unwrap_or(0),
        retry_jobs: u64::try_from(retry.max(0)).unwrap_or(0),
        last_error,
    })
}
