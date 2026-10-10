// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres implementation of the dense
//! index-coverage counts (visible indexable items vs derived rows and queue).
//!
//! ## Owns
//! - `index_coverage_pg`: portable-semantics coverage counts for one scope
//!
//! ## Does not own
//! - The `IndexStore` contract or the queue/lexical SQL (see `postgres_index`)
//! - Coverage counting for SQLite (see `sqlite_coverage`)
//!
//! ## Boundary
//!
//! Changes to Postgres coverage SQL should normally require changes here.
//! Changes to search primitives or the queue should not require changes here.
//!
//! Keep this module focused on Postgres coverage counts.

use crate::migrate::IntoAmError;
use crate::model::IndexCoverage;
use clio_types::AmError;

/// Coverage counts (portable semantics: visible indexable items vs derived rows).
pub(crate) fn index_coverage_pg(
    client: &mut postgres::Client,
    bank_id: Option<&str>,
) -> Result<IndexCoverage, AmError> {
    let bank_where = match bank_id {
        Some(_) => " AND items.bank_id = $1",
        None => "",
    };
    let count = |client: &mut postgres::Client, sql: &str| -> Result<i64, AmError> {
        if bank_id.is_some() {
            let row = client.query_one(sql, &[&bank_id]).am("coverage count")?;
            Ok(row.get::<_, i64>(0))
        } else {
            let n = client.query_one(sql, &[]).am("coverage count")?;
            Ok(n.get::<_, i64>(0))
        }
    };
    let indexable = count(
        client,
        &format!(
            "SELECT COUNT(*) FROM items
             WHERE items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
        ),
    )?;
    let dense = count(
        client,
        &format!(
            "SELECT COUNT(*) FROM item_embeddings e
             JOIN items ON items.id = e.item_id AND items.bank_id = e.bank_id
             WHERE items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
        ),
    )?;
    let lexical = count(
        client,
        &format!(
            "SELECT COUNT(*) FROM items
             WHERE items.content_tsv IS NOT NULL
               AND items.tx_until IS NULL AND items.discarded_at IS NULL{bank_where}"
        ),
    )?;
    let (pending, retry, last_error): (i64, i64, Option<String>) = if bank_id.is_some() {
        let row = client
            .query_one(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE attempts > 0),
                        (SELECT last_error FROM index_pending
                          WHERE bank_id = $1 AND last_error IS NOT NULL
                          ORDER BY updated_at DESC LIMIT 1)
                 FROM index_pending WHERE bank_id = $1",
                &[&bank_id],
            )
            .am("coverage queue")?;
        (row.get(0), row.get(1), row.get(2))
    } else {
        let row = client
            .query_one(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE attempts > 0),
                        (SELECT last_error FROM index_pending
                          WHERE last_error IS NOT NULL
                          ORDER BY updated_at DESC LIMIT 1)
                 FROM index_pending",
                &[],
            )
            .am("coverage queue")?;
        (row.get(0), row.get(1), row.get(2))
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
