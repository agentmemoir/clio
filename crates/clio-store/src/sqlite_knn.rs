// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Bank-scoped sqlite-vec KNN (extension cannot filter aux columns in MATCH).

use crate::migrate::IntoAmError;
use crate::model::KnnHit;
use clio_types::AmError;
use rusqlite::{Connection, params};
use zerocopy::IntoBytes;

/// Exact in-bank top-k: widen the global fetch until the bank is filled or exhausted.
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
        let mut fetched = 0usize;
        let mut out = Vec::new();
        for row in rows {
            let (item_id, distance, row_bank) = row.am("knn row")?;
            fetched += 1;
            if row_bank == bank_id {
                out.push(KnnHit { item_id, distance });
                if out.len() >= k {
                    return Ok(out);
                }
            }
        }
        if fetched < fetch_k {
            return Ok(out);
        }
        // Table sizes at this store generation stay far below usize overflow; expect holds.
        fetch_k = fetch_k
            .checked_mul(2)
            .expect("knn fetch_k overflow — raise the bank-filter strategy");
    }
}
