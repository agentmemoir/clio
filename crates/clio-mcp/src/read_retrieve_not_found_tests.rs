// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the association and graph readers name an unknown item as
//! not found, and that a stored item with no edges still reads as an empty result.
//!
//! ## Owns
//! - Assertions over `associations` and `graph_query` for unknown and stored item ids
//!
//! ## Does not own
//! - Edge weights and traversal (see the other `read_retrieve` test modules)
//!
//! ## Boundary
//!
//! Changes to how a missing item is reported should normally require changes here.
//! Changes to edge scoring should not.
//!
//! Keep this module focused on the association and graph readers' not-found rule.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};

fn state() -> std::sync::Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_item(state: &McpState, id: &str) {
    let out = call(
        state,
        "store",
        json!({
            "item": {
                "id": id, "subject_id": id, "gist": format!("terse answers {id}"),
                "snapshot": {"entity": format!("terse answers {id}"), "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona", "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
            "source_text": format!("The user prefers terse answers {id} (1 note since 2024-01-01).")
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn associations_of_an_unknown_item_is_item_not_found() {
    let s = state();
    let out = call(
        &s,
        "associations",
        json!({"item_id": "itm-nope", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("item_not_found"), "{out}");
}

#[test]
fn graph_query_from_an_unknown_seed_is_item_not_found() {
    let s = state();
    let out = call(
        &s,
        "graph_query",
        json!({"seed_id": "itm-nope", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("item_not_found"), "{out}");
}

#[test]
fn associations_of_a_stored_item_with_no_edges_is_empty() {
    let s = state();
    store_item(&s, "itm-assoc-1");
    let out = call(
        &s,
        "associations",
        json!({"item_id": "itm-assoc-1", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        out["associations"].as_array().map(Vec::len),
        Some(0),
        "{out}"
    );
}
