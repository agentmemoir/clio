// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bank-scoped sqlite-vec KNN scan and its
//! current-belief visibility filter.
//!
//! ## Owns
//! - `knn_in_bank`: the widened vec0 scan plus the visibility post-filter
//! - The batch visibility query against `items`
//!
//! ## Does not own
//! - Domain filtering or the surrounding limit widening (`sqlite_index`)
//! - Postgres KNN SQL (`postgres_store`)
//!
//! ## Boundary
//!
//! Changes to SQLite KNN candidate selection should normally require changes
//! here. Changes to index-level domain filtering should not require changes here.
//!
//! Keep this module focused on the SQLite KNN scan and visibility filter.

use crate::migrate::IntoAmError;
use crate::model::KnnHit;
use clio_types::AmError;
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeSet;
use zerocopy::IntoBytes;

/// Exact in-bank top-k over currently visible items.
///
/// The vec0 extension cannot filter auxiliary columns inside `MATCH`, so the
/// scan widens until `k` visible in-bank rows are collected or the scan is
/// exhausted. Visibility is the index legs' current-belief rule: the item is
/// the current transaction belief (`tx_until IS NULL`), not discarded, and not
/// archived. A corrected prior keeps its vector row but never returns.
pub(crate) fn knn_in_bank(
    conn: &Connection,
    bank_id: &str,
    query: &[f32],
    k: usize,
) -> Result<Vec<KnnHit>, AmError> {
    // Start at `k` (not a large fixed over-fetch) so multi-bank cases must widen.
    let mut fetch_k = k.max(1);
    loop {
        let mut stmt = conn
            .prepare(
                "SELECT item_id, distance, bank_id
                 FROM item_embeddings
                 WHERE embedding MATCH ?1 AND k = ?2
                 ORDER BY distance",
            )
            .am("knn prepare")?;
        let rows = stmt
            .query_map(params![query.as_bytes(), fetch_k as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .am("knn query_map")?;
        let mut scanned = 0usize;
        let mut in_bank = Vec::new();
        for row in rows {
            let (item_id, distance, row_bank) = row.am("knn row")?;
            scanned += 1;
            if row_bank == bank_id {
                in_bank.push(KnnHit { item_id, distance });
            }
        }
        let visible = visible_item_ids(conn, bank_id, &in_bank)?;
        let out: Vec<KnnHit> = in_bank
            .into_iter()
            .filter(|hit| visible.contains(&hit.item_id))
            .take(k)
            .collect();
        if out.len() >= k || scanned < fetch_k {
            return Ok(out);
        }
        // Table sizes at this store generation stay far below usize overflow; expect holds.
        fetch_k = fetch_k
            .checked_mul(2)
            .expect("knn fetch_k overflow — raise the bank-filter strategy");
    }
}

/// The ids in `hits` whose item rows are currently visible, in one query.
fn visible_item_ids(
    conn: &Connection,
    bank_id: &str,
    hits: &[KnnHit],
) -> Result<BTreeSet<String>, AmError> {
    if hits.is_empty() {
        return Ok(BTreeSet::new());
    }
    let placeholders = vec!["?"; hits.len()].join(", ");
    let sql = format!(
        "SELECT id FROM items
         WHERE bank_id = ? AND discarded_at IS NULL AND tx_until IS NULL
           AND archived_at IS NULL AND id IN ({placeholders})"
    );
    let mut binds: Vec<Value> = Vec::with_capacity(hits.len() + 1);
    binds.push(Value::Text(bank_id.to_owned()));
    for hit in hits {
        binds.push(Value::Text(hit.item_id.clone()));
    }
    let mut stmt = conn.prepare(&sql).am("knn visibility prepare")?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), |r| r.get::<_, String>(0))
        .am("knn visibility query")?;
    let mut visible = BTreeSet::new();
    for row in rows {
        visible.insert(row.am("knn visibility row")?);
    }
    Ok(visible)
}

#[cfg(test)]
#[path = "sqlite_knn_tests.rs"]
pub(crate) mod sqlite_knn_tests;
