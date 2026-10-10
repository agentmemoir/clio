// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite MemTree node CRUD.
//!
//! ## Owns
//! - Upsert / get / query of `memtree_nodes` on SQLite
//!
//! ## Does not own
//! - Tree split / dirty-path algorithms
//!
//! ## Boundary
//!
//! Changes to SQLite MemTree SQL should normally require changes here.
//! Changes to Postgres MemTree SQL should not require changes here.
//!
//! Keep this module focused on SQLite MemTree persistence.

use crate::memtree_model::{MemTreeNodeRecord, MemTreeQuery};
use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};

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

fn map_row(row: &rusqlite::Row<'_>) -> Result<MemTreeNodeRecord, rusqlite::Error> {
    let child_raw: String = row.get(7)?;
    let child_ids = decode_children(&child_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(MemTreeNodeRecord {
        id: row.get(0)?,
        bank_id: row.get(1)?,
        parent_id: row.get(2)?,
        item_id: row.get(3)?,
        depth: row.get(4)?,
        interval_start: row.get(5)?,
        interval_end: row.get(6)?,
        child_ids,
        summary_gist: row.get(8)?,
        dirty: row.get::<_, i64>(9)? != 0,
        generation: row.get(10)?,
        refreshing: row.get::<_, i64>(11)? != 0,
        task_id: row.get(12)?,
        last_error: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

const SELECT_COLS: &str = "id, bank_id, parent_id, item_id, depth, interval_start, interval_end,
                child_ids_json, summary_gist, dirty, generation, refreshing, task_id,
                last_error, created_at, updated_at";

/// Insert or replace a MemTree node (bank-scoped).
pub(crate) fn upsert(conn: &Connection, node: &MemTreeNodeRecord) -> Result<(), AmError> {
    if node.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let children = encode_children(&node.child_ids);
    conn.execute(
        "INSERT INTO memtree_nodes (
           id, bank_id, parent_id, item_id, depth, interval_start, interval_end,
           child_ids_json, summary_gist, dirty, generation, refreshing, task_id,
           last_error, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(id) DO UPDATE SET
           bank_id=excluded.bank_id,
           parent_id=excluded.parent_id,
           item_id=excluded.item_id,
           depth=excluded.depth,
           interval_start=excluded.interval_start,
           interval_end=excluded.interval_end,
           child_ids_json=excluded.child_ids_json,
           summary_gist=excluded.summary_gist,
           dirty=excluded.dirty,
           generation=excluded.generation,
           refreshing=excluded.refreshing,
           task_id=excluded.task_id,
           last_error=excluded.last_error,
           updated_at=excluded.updated_at
         WHERE memtree_nodes.bank_id = excluded.bank_id",
        params![
            node.id,
            node.bank_id,
            node.parent_id,
            node.item_id,
            node.depth,
            node.interval_start,
            node.interval_end,
            children,
            node.summary_gist,
            i64::from(node.dirty),
            node.generation,
            i64::from(node.refreshing),
            node.task_id,
            node.last_error,
            node.created_at,
            node.updated_at,
        ],
    )
    .am("memtree upsert")?;
    Ok(())
}

/// Read one node scoped by bank.
pub(crate) fn get(
    conn: &Connection,
    bank_id: &str,
    node_id: &str,
) -> Result<Option<MemTreeNodeRecord>, AmError> {
    conn.query_row(
        &format!("SELECT {SELECT_COLS} FROM memtree_nodes WHERE id = ?1 AND bank_id = ?2"),
        params![node_id, bank_id],
        map_row,
    )
    .optional()
    .am("memtree get")
}

/// Query nodes with optional filters (bank required).
pub(crate) fn query(
    conn: &Connection,
    q: &MemTreeQuery,
) -> Result<Vec<MemTreeNodeRecord>, AmError> {
    if q.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let limit = i64::try_from(q.limit.unwrap_or(100).min(1000)).unwrap_or(100);
    // Fetch bank-scoped rows then filter in-process (keeps SQL portable/simple).
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_COLS} FROM memtree_nodes WHERE bank_id = ?1
             ORDER BY interval_start, id"
        ))
        .am("memtree query prepare")?;
    let rows = stmt
        .query_map(params![q.bank_id], map_row)
        .am("memtree query")?;
    let mut out = Vec::new();
    for row in rows {
        let n = row.am("memtree query row")?;
        if let Some(ref start) = q.time_start
            && n.interval_end.as_str() < start.as_str()
        {
            continue;
        }
        if let Some(ref end) = q.time_end
            && n.interval_start.as_str() > end.as_str()
        {
            continue;
        }
        if let Some(depth) = q.depth
            && n.depth != depth
        {
            continue;
        }
        if let Some(ref task) = q.task_id
            && n.task_id.as_deref() != Some(task.as_str())
        {
            continue;
        }
        out.push(n);
        if out.len() as i64 >= limit {
            break;
        }
    }
    Ok(out)
}
