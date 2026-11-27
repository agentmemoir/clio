// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `graph_query` hop-cap argument tests: the
//! cap value itself is allowed, and everything else the caller can send for
//! `max_hops` is refused rather than answered at the default.
//!
//! ## Owns
//! - The accepted cap, the refused value above it, and the refused value that is
//!   not a whole non-negative number
//!
//! ## Does not own
//! - Traversal semantics and the seeded walk (see `clio-retrieve`'s
//!   `assoc_graph_tests`)
//! - The not-found rule of an unknown seed (see `read_retrieve_not_found_tests`)
//!
//! ## Boundary
//!
//! Changes to which `max_hops` values are accepted should normally require
//! changes here. Changes to how a walk is ranked should not.
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_item(state: &McpState) {
    let out = call(
        state,
        "store",
        json!({
            "item": {
                "id": "itm-1", "subject_id": "subj", "gist": "terse answers",
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec", "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
            "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn graph_query_allows_the_cap_hop_count() {
    let s = state();
    store_item(&s);
    // The cap value itself is inside the cap, so it must be answered, not refused.
    let out = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": 3}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["nodes"].is_array(), "{out}");
}

#[test]
fn graph_query_refuses_the_u32_max_hop_count_with_the_real_cap() {
    let s = state();
    store_item(&s);
    // `u32::MAX` fits `u64` and so reaches the cap check; it is still too far.
    let out = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": 4_294_967_295u64}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"), "{out}");
    assert_eq!(
        out["message"],
        json!("graph_query max_hops must be <= 3"),
        "the refusal must name the cap: {out}"
    );
}

#[test]
fn graph_query_refuses_a_fractional_max_hops() {
    let s = state();
    store_item(&s);
    // A float passes the schema's `number` check, so the handler must refuse it
    // instead of reading it as an omitted argument.
    let out = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": 2.5}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"), "{out}");
}

#[test]
fn graph_query_refuses_a_negative_max_hops() {
    let s = state();
    store_item(&s);
    // A negative number passes the schema's `number` check, so the handler must
    // refuse it instead of reading it as an omitted argument.
    let out = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": -1}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"), "{out}");
}
