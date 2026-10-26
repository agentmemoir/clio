// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! MemTree node/row conversion coverage.
//!
//! Test-only. Never compiled into production builds.

use crate::memtree::{MemTreeNode, MemTreeQueryFilter};
use crate::memtree_record::{node_from_record, query_from_filter, record_from_node};

fn leaf() -> MemTreeNode {
    MemTreeNode {
        id: "bank-a:leaf:1".into(),
        bank_id: "bank-a".into(),
        parent_id: Some("bank-a:int:2".into()),
        item_id: Some("item-1".into()),
        depth: 0,
        interval_start: "2026-06-01T00:00:00Z".into(),
        interval_end: "2026-06-01T00:00:00Z".into(),
        child_ids: vec![],
        summary_gist: Some("gist one".into()),
        dirty: false,
        generation: 3,
        refreshing: false,
        task_id: Some("task-1".into()),
        last_error: None,
    }
}

#[test]
fn roundtrip_preserves_every_node_column() {
    let node = leaf();
    let back = node_from_record(record_from_node(&node, "2026-06-02T00:00:00Z"));
    assert_eq!(back, node);
}

#[test]
fn internal_node_roundtrip_keeps_child_order() {
    let mut node = leaf();
    node.id = "bank-a:int:9".into();
    node.item_id = None;
    node.depth = 2;
    node.child_ids = vec!["bank-a:int:7".into(), "bank-a:int:8".into()];
    node.summary_gist = None;
    node.dirty = true;
    node.last_error = Some("refresh failed".into());
    let back = node_from_record(record_from_node(&node, "t0"));
    assert_eq!(back.child_ids, node.child_ids);
    assert_eq!(back, node);
}

#[test]
fn record_stamps_both_time_columns_with_the_write_time() {
    let record = record_from_node(&leaf(), "2026-06-02T00:00:00Z");
    assert_eq!(record.created_at, "2026-06-02T00:00:00Z");
    assert_eq!(record.updated_at, "2026-06-02T00:00:00Z");
}

#[test]
fn depths_and_generations_beyond_the_column_width_saturate() {
    let mut node = leaf();
    node.depth = u32::MAX;
    node.generation = u64::MAX;
    let record = record_from_node(&node, "t0");
    assert_eq!(record.depth, i32::MAX, "depth widened without wrapping");
    assert_eq!(
        record.generation,
        i64::MAX,
        "generation widened without wrapping"
    );
}

#[test]
fn query_from_filter_scopes_the_bank_and_widens_the_depth() {
    let filter = MemTreeQueryFilter {
        time_start: Some("2026-06-01T00:00:00Z".into()),
        time_end: Some("2026-07-01T00:00:00Z".into()),
        depth: Some(2),
        task_id: Some("task-1".into()),
        limit: Some(5),
    };
    let query = query_from_filter("bank-a", &filter);
    assert_eq!(query.bank_id, "bank-a");
    assert_eq!(query.depth, Some(2));
    assert_eq!(query.time_start, filter.time_start);
    assert_eq!(query.time_end, filter.time_end);
    assert_eq!(query.task_id, filter.task_id);
    assert_eq!(query.limit, Some(5));
}
