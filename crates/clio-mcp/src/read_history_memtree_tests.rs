// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `memtree_query` / `memtree_get` read
//! tests: the nodes a stored item's leaf attach creates must be readable
//! through both handlers, and both must refuse what the bank does not hold.
//!
//! ## Owns
//! - The `memtree_query` argument pass-through (`time_range`, `depth`, `limit`,
//!   `task_id`) and its depth cap
//! - The not-found shape of an empty query and of an unknown node id
//!
//! ## Does not own
//! - The handlers under test (see `read_history`)
//! - Store-first reads, the forest fallback, and record conversion (see
//!   `clio-write`'s `memtree_tools_tests`)
//!
//! ## Boundary
//!
//! Changes to how a `MemTree` read is answered should normally require changes
//! here. Changes to how a node is persisted should not.
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_write::MemTreeQueryFilter;
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "bank-a";

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one item and return its state plus the id of a node its leaf attach
/// created, read straight off the forest so the fixture does not borrow the
/// handlers under test. The ids are sorted before the pick, so the fixture does
/// not depend on the tree's iteration order.
fn stored() -> (Arc<McpState>, String) {
    let state = state();
    let out = call(
        &state,
        "store",
        json!({
            "item": {
                "id": "itm-1",
                "subject_id": "subj",
                "gist": "terse answers",
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
            "bank": BANK,
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "store itm-1: {out}");
    let node_id = {
        let guard = state.forest.lock().expect("forest");
        let mut ids: Vec<String> = guard
            .tree(BANK)
            .expect("bank tree")
            .query(&MemTreeQueryFilter::default())
            .into_iter()
            .map(|node| node.id)
            .collect();
        ids.sort_unstable();
        ids.first().expect("a node from the leaf attach").clone()
    };
    (state, node_id)
}

#[test]
fn memtree_query_returns_the_node_a_stored_item_created() {
    let (state, node_id) = stored();
    let out = call(&state, "memtree_query", json!({"bank": BANK}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let readable = out["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .any(|id| id == node_id);
    assert!(
        readable,
        "the leaf attach node must be readable through memtree_query: {out}"
    );
}

#[test]
fn memtree_query_passes_the_time_range_depth_and_limit_through() {
    let (state, _) = stored();
    let out = call(
        &state,
        "memtree_query",
        json!({
            "bank": BANK,
            "time_range": {"start": "2020-01-01", "end": "2030-01-01"},
            "depth": 0,
            "limit": 5
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        !out["nodes"].as_array().expect("nodes").is_empty(),
        "a leaf inside the range must be returned: {out}"
    );
}

#[test]
fn memtree_query_for_a_task_no_node_carries_is_not_found() {
    let (state, _) = stored();
    let out = call(
        &state,
        "memtree_query",
        json!({"bank": BANK, "task_id": "tsk-nope"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
}

#[test]
fn memtree_query_refuses_a_depth_above_its_cap() {
    let (state, _) = stored();
    let out = call(&state, "memtree_query", json!({"bank": BANK, "depth": 99}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"), "{out}");
    assert!(
        out["message"].as_str().unwrap_or_default().contains("32"),
        "the refusal must name the cap: {out}"
    );
}

#[test]
fn memtree_get_reads_the_node_a_stored_item_created() {
    let (state, node_id) = stored();
    let out = call(
        &state,
        "memtree_get",
        json!({"bank": BANK, "node_id": node_id}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["id"], json!(node_id), "{out}");
}

#[test]
fn memtree_get_of_an_unknown_node_is_not_found() {
    let (state, _) = stored();
    let out = call(
        &state,
        "memtree_get",
        json!({"bank": BANK, "node_id": "nd-ghost"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nd-ghost"),
        "the refusal must name the missing node: {out}"
    );
}
