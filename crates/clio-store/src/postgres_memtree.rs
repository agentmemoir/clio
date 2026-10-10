// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres MemTree node CRUD.
//!
//! ## Owns
//! - Upsert / get / query of `memtree_nodes` on Postgres
//!
//! ## Does not own
//! - Tree split / dirty-path algorithms
//!
//! ## Boundary
//!
//! Changes to Postgres MemTree SQL should normally require changes here.
//! Changes to SQLite MemTree SQL should not require changes here.
//!
//! Keep this module focused on Postgres MemTree persistence.

use crate::memtree_model::{MemTreeNodeRecord, MemTreeQuery};
use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};
use postgres::Client;

fn encode_children(ids: &[String]) -> String {
    serde_json::to_string(ids).expect("child id list serializes")
}

fn decode_children(raw: &str) -> Result<Vec<String>, AmError> {
    serde_json::from_str(raw)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("memtree child_ids_json: {e}")))
}

#[cfg(test)]
pub(crate) fn decode_children_for_test(raw: &str) -> Result<Vec<String>, AmError> {
    decode_children(raw)
}

fn map_row(row: &postgres::Row) -> Result<MemTreeNodeRecord, AmError> {
    let child_raw: String = row.get(7);
    Ok(MemTreeNodeRecord {
        id: row.get(0),
        bank_id: row.get(1),
        parent_id: row.get(2),
        item_id: row.get(3),
        depth: row.get(4),
        interval_start: row.get(5),
        interval_end: row.get(6),
        child_ids: decode_children(&child_raw)?,
        summary_gist: row.get(8),
        dirty: row.get::<_, i32>(9) != 0,
        generation: i64::from(row.get::<_, i32>(10)),
        refreshing: row.get::<_, i32>(11) != 0,
        task_id: row.get(12),
        last_error: row.get(13),
        created_at: row.get(14),
        updated_at: row.get(15),
    })
}

/// Insert or replace a MemTree node (bank-scoped).
pub(crate) fn upsert(client: &mut Client, node: &MemTreeNodeRecord) -> Result<(), AmError> {
    if node.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let children = encode_children(&node.child_ids);
    let dirty = i32::from(node.dirty);
    let refreshing = i32::from(node.refreshing);
    let generation = i32::try_from(node.generation).unwrap_or(i32::MAX);
    client
        .execute(
            "INSERT INTO memtree_nodes (
               id, bank_id, parent_id, item_id, depth, interval_start, interval_end,
               child_ids_json, summary_gist, dirty, generation, refreshing, task_id,
               last_error, created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
             ON CONFLICT (id) DO UPDATE SET
               bank_id = EXCLUDED.bank_id,
               parent_id = EXCLUDED.parent_id,
               item_id = EXCLUDED.item_id,
               depth = EXCLUDED.depth,
               interval_start = EXCLUDED.interval_start,
               interval_end = EXCLUDED.interval_end,
               child_ids_json = EXCLUDED.child_ids_json,
               summary_gist = EXCLUDED.summary_gist,
               dirty = EXCLUDED.dirty,
               generation = EXCLUDED.generation,
               refreshing = EXCLUDED.refreshing,
               task_id = EXCLUDED.task_id,
               last_error = EXCLUDED.last_error,
               updated_at = EXCLUDED.updated_at
             WHERE memtree_nodes.bank_id = EXCLUDED.bank_id",
            &[
                &node.id,
                &node.bank_id,
                &node.parent_id,
                &node.item_id,
                &node.depth,
                &node.interval_start,
                &node.interval_end,
                &children,
                &node.summary_gist,
                &dirty,
                &generation,
                &refreshing,
                &node.task_id,
                &node.last_error,
                &node.created_at,
                &node.updated_at,
            ],
        )
        .am("memtree upsert pg")?;
    Ok(())
}

/// Read one node scoped by bank.
pub(crate) fn get(
    client: &mut Client,
    bank_id: &str,
    node_id: &str,
) -> Result<Option<MemTreeNodeRecord>, AmError> {
    let rows = client
        .query(
            "SELECT id, bank_id, parent_id, item_id, depth, interval_start, interval_end,
                    child_ids_json, summary_gist, dirty, generation, refreshing, task_id,
                    last_error, created_at, updated_at
             FROM memtree_nodes WHERE id = $1 AND bank_id = $2",
            &[&node_id, &bank_id],
        )
        .am("memtree get pg")?;
    rows.first().map(map_row).transpose()
}

/// Query nodes with optional filters (bank required).
pub(crate) fn query(
    client: &mut Client,
    q: &MemTreeQuery,
) -> Result<Vec<MemTreeNodeRecord>, AmError> {
    if q.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let limit = i64::try_from(q.limit.unwrap_or(100).min(1000)).unwrap_or(100);
    let rows = client
        .query(
            "SELECT id, bank_id, parent_id, item_id, depth, interval_start, interval_end,
                    child_ids_json, summary_gist, dirty, generation, refreshing, task_id,
                    last_error, created_at, updated_at
             FROM memtree_nodes
             WHERE bank_id = $1
               AND ($2::text IS NULL OR interval_end >= $2)
               AND ($3::text IS NULL OR interval_start <= $3)
               AND ($4::int IS NULL OR depth = $4)
               AND ($5::text IS NULL OR task_id = $5)
             ORDER BY interval_start, id
             LIMIT $6",
            &[
                &q.bank_id,
                &q.time_start,
                &q.time_end,
                &q.depth,
                &q.task_id,
                &limit,
            ],
        )
        .am("memtree query pg")?;
    rows.iter().map(map_row).collect()
}
