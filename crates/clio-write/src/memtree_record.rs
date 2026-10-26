// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for converting between in-memory MemTree nodes and
//! their durable store rows and queries.
//!
//! ## Owns
//! - `MemTreeNode` -> `MemTreeNodeRecord` and the reverse conversion
//! - `MemTreeQueryFilter` -> bank-scoped `MemTreeQuery`
//! - Widening the narrower columns (`depth` to `i32`, `generation` to `i64`) and
//!   the write-time stamp the row carries
//!
//! ## Does not own
//! - The insert / conflict statements that decide which columns a repeat write
//!   may move (`sqlite_memtree`, `postgres_memtree` in `clio-store`)
//! - Attach, dirty marking, or summary refresh algorithms
//!
//! ## Boundary
//!
//! Changes to how a node maps onto its row should normally require changes here.
//! Changes to the SQL that writes the row should not require changes here.
//!
//! Keep this module focused on the node/row shape conversion.

use crate::memtree::{MemTreeNode, MemTreeQueryFilter};
use clio_store::{MemTreeNodeRecord, MemTreeQuery};

/// The durable row for an in-memory node, stamped with the write time.
///
/// `created_at` and `updated_at` both carry `now`. The node itself has no
/// creation time, so the first write of a node establishes it and later writes
/// must not move it: both backends leave `created_at` out of their conflict
/// update, so only this row's first stored value survives. A caller therefore
/// passes the write time and needs no history to do it.
///
/// `child_ids` stays a `Vec<String>`: each backend serializes it into the
/// `child_ids_json` text column itself, so this conversion must not pre-encode.
#[must_use]
pub fn record_from_node(node: &MemTreeNode, now: &str) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: node.id.clone(),
        bank_id: node.bank_id.clone(),
        parent_id: node.parent_id.clone(),
        item_id: node.item_id.clone(),
        depth: i32::try_from(node.depth).unwrap_or(i32::MAX),
        interval_start: node.interval_start.clone(),
        interval_end: node.interval_end.clone(),
        child_ids: node.child_ids.clone(),
        summary_gist: node.summary_gist.clone(),
        dirty: node.dirty,
        generation: i64::try_from(node.generation).unwrap_or(i64::MAX),
        refreshing: node.refreshing,
        task_id: node.task_id.clone(),
        last_error: node.last_error.clone(),
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    }
}

/// The in-memory node a stored row describes.
///
/// Narrowing is saturating for the same reason widening is: a row this build
/// cannot represent must still rebuild the forest instead of aborting it. The
/// stored `created_at` / `updated_at` have no in-memory counterpart.
#[must_use]
pub fn node_from_record(record: MemTreeNodeRecord) -> MemTreeNode {
    MemTreeNode {
        id: record.id,
        bank_id: record.bank_id,
        parent_id: record.parent_id,
        item_id: record.item_id,
        depth: u32::try_from(record.depth).unwrap_or(0),
        interval_start: record.interval_start,
        interval_end: record.interval_end,
        child_ids: record.child_ids,
        summary_gist: record.summary_gist,
        dirty: record.dirty,
        generation: u64::try_from(record.generation).unwrap_or(0),
        refreshing: record.refreshing,
        task_id: record.task_id,
        last_error: record.last_error,
    }
}

/// The store query for a bank-scoped read filter.
#[must_use]
pub fn query_from_filter(bank_id: &str, filter: &MemTreeQueryFilter) -> MemTreeQuery {
    MemTreeQuery {
        bank_id: bank_id.to_owned(),
        time_start: filter.time_start.clone(),
        time_end: filter.time_end.clone(),
        depth: filter.depth.map(|d| i32::try_from(d).unwrap_or(i32::MAX)),
        task_id: filter.task_id.clone(),
        limit: filter.limit,
    }
}

#[cfg(test)]
#[path = "memtree_record_tests.rs"]
mod memtree_record_tests;
