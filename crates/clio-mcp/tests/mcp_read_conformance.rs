// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for direct-dispatch read conformance over the
//! Read set: tool listing, the gate/explicit-retrieve split, context
//! budgeting, snapshot-vs-gist authority, bi-temporal reads, co-activation
//! durability, and the structured negative/bank-scoping contract
//! The Core read surface those tests exist to pin.
//!
//! ## Owns
//! - `ac_17_01_all_read_tools_listed` and the `t17_01..t17_06` read scenarios
//! - `negatives_are_structured_and_bank_scoped`
//!
//! ## Does not own
//! - Transport-level parity matrices and stdout purity
//!   (`mcp_read_transport_conformance`)
//! - The shared fixture state (see the `mcp_read_support` module)
//!
//! ## Boundary
//!
//! Changes to direct read-tool semantics should normally require changes here.
//! Changes to transport framing or parity masking should not require changes here.
//!
//! Keep this module focused on direct-dispatch read conformance.

#[path = "mcp_read_support/mod.rs"]
mod support;

use clio_mcp::bound_read_tools;
use clio_mcp::protocol::McpHandler;
use serde_json::json;

use support::{BANK, read_state};

/// Every Core read tool is advertised with an object inputSchema.
#[test]
fn ac_17_01_all_read_tools_listed() {
    let handler = McpHandler::new(read_state());
    let res = handler
        .handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .expect("response");
    let tools = res["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for bound in bound_read_tools() {
        assert!(names.contains(bound), "`{bound}` missing from tools/list");
    }
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], json!("object"));
    }
}

/// Gate skips a greeting; explicit retrieve still runs.
#[test]
fn t17_01_t17_02_gate_skip_and_explicit_retrieve() {
    let state = read_state();
    let gate = clio_mcp::write_tools::dispatch(&state, "intent_gate", &json!({"turn_text": "hi"}));
    assert_eq!(gate["retrieval_needed"], json!(false), "{gate}");
    let out = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"query": "terse answers", "explain": true}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let hits = out["hits"].as_array().expect("hits");
    assert!(hits.len() >= 2, "explicit retrieve must run: {out}");
    assert!(out["explanation"]["hits"].is_array());
}

/// `compose_context` never exceeds the requested token budget.
#[test]
fn t17_03_compose_budget_is_enforced() {
    let state = read_state();
    let out = clio_mcp::write_tools::dispatch(
        &state,
        "compose_context",
        &json!({"query": "terse answers", "budget_tokens": 300}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out["total_tokens"].as_u64().unwrap_or(u64::MAX) <= 300,
        "budget exceeded: {out}"
    );
}

/// snapshot is lossless; gist is flagged non-authoritative.
#[test]
fn t17_04_snapshot_vs_gist() {
    let state = read_state();
    let snap = clio_mcp::write_tools::dispatch(&state, "get_snapshot", &json!({"item_id": "cf-1"}));
    assert_eq!(snap["snapshot"]["amount"], json!(1));
    assert_eq!(snap["authoritative"], json!(true));
    let gist = clio_mcp::write_tools::dispatch(&state, "get_gist", &json!({"item_id": "cf-1"}));
    assert_eq!(gist["gist"], json!("terse answers cf-1"));
    assert_eq!(gist["authoritative"], json!(false));
}

/// bi-temporal triple query ignores later supersede at historical `as_of`.
#[test]
fn t17_05_triple_query_honors_as_of() {
    let state = read_state();
    let second = clio_mcp::write_tools::dispatch(
        &state,
        "triple_add",
        &json!({"bank": BANK, "subject": "rust", "predicate": "used_by", "object": "compiler",
                 "epistemic_kind": "fact", "valid_from": "2024-06-01T00:00:00Z"}),
    );
    assert_eq!(second["ok"], json!(true), "{second}");
    let historical = clio_mcp::write_tools::dispatch(
        &state,
        "triple_query",
        &json!({"bank": BANK, "subject": "rust", "predicate": "used_by",
                 "as_of": "2024-03-01T00:00:00Z", "time_axis": "valid"}),
    );
    let triples = historical["triples"].as_array().expect("triples");
    assert_eq!(triples.len(), 1, "{historical}");
    assert_eq!(triples[0]["object"], json!("agent"));
}

/// `retrieve` durably persists co-activation edges.
#[test]
fn t17_06_retrieve_persists_assoc_edges() {
    let state = read_state();
    let before = state
        .store
        .list_assoc_edges(BANK, None)
        .expect("edges before")
        .len();
    let out = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "terse answers"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let after = state
        .store
        .list_assoc_edges(BANK, None)
        .expect("edges after");
    assert!(
        after.len() > before,
        "assoc_edge weight must be persisted after retrieve"
    );
}

/// Negative: missing items, deep traversal, bad `as_of`, cross-bank scoping.
#[test]
fn negatives_are_structured_and_bank_scoped() {
    let state = read_state();
    let missing = clio_mcp::write_tools::dispatch(&state, "get", &json!({"item_id": "ghost"}));
    assert_eq!(missing["code"], json!("item_not_found"));

    let deep = clio_mcp::write_tools::dispatch(
        &state,
        "graph_query",
        &json!({"seed_id": "cf-1", "max_hops": 99}),
    );
    assert_eq!(deep["code"], json!("out_of_range"));

    let bad_axis = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"query": "terse answers", "time_axis": "sideways"}),
    );
    assert_eq!(bad_axis["code"], json!("invalid_argument"));

    // Cross-bank read returns nothing rather than leaking.
    let other = clio_mcp::write_tools::dispatch(&state, "inspect", &json!({"bank": "other-bank"}));
    assert_eq!(other["ok"], json!(true), "{other}");
    let items = other["items"].as_array().expect("items");
    assert!(items.is_empty(), "{items:?}");
    let cross = clio_mcp::write_tools::dispatch(
        &state,
        "get",
        &json!({"bank": "other-bank", "item_id": "cf-1"}),
    );
    assert_eq!(cross["code"], json!("item_not_found"));
}
