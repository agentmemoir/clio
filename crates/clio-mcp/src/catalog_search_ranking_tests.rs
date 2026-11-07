// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `tool_search` ranking: the tie
//! that keeps catalog order, determinism, the family filter, the clamped limit,
//! and reachability of every catalog tool through the dispatched path.
//!
//! ## Owns
//! - An equal-score fixture ordered by the catalog, not the alphabet or groups
//! - Two identical calls returning one order
//! - A family page that is the unfiltered order with other families removed
//! - A limit above the maximum clamping to 25
//! - Every catalog tool found by its own name and description at limit 25
//!
//! ## Does not own
//! - Tokenizer, score, validation, or the gate (`catalog_search_tests`)
//! - Catalog membership or group data (`clio-types` tests)
//!
//! ## Boundary
//!
//! Changes to rank order, ties, or reachability should normally require changes
//! here. Changes to an argument's validation should not.
//!
//! Keep this module focused on the order a search page holds.

use super::{NAME, score, tokens};
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use crate::schema::catalog_defs;
use clio_types::{GROUP_TABLE, ToolGroup, group_of};
use serde_json::{Value, json};
use std::sync::Arc;

/// A server state whose configuration layer is exactly `config`.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// A gate-on state, which is the only state that answers `tool_search`.
fn searchable() -> Arc<McpState> {
    state_with(r#"{"mcp": {"discovery": true}}"#)
}

/// The whole response to one `tool_search` call through the dispatcher.
fn dispatched(state: &Arc<McpState>, args: &Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": NAME, "arguments": args }
    });
    McpHandler::new(Arc::clone(state))
        .handle_message(&request)
        .expect("a request with an id has a response")
}

/// The tool names of one search page, in page order.
fn page(state: &Arc<McpState>, args: &Value) -> Vec<String> {
    let response = dispatched(state, args);
    response["result"]["structuredContent"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("a search page for {args}: {response}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The ranking the contract predicts: descending raw score over the catalog
/// order, which is the stable tie-break.
fn expected_ranking(query: &str, limit: usize) -> Vec<String> {
    let query_tokens = tokens(query);
    let mut scored: Vec<(usize, String)> = catalog_defs()
        .into_iter()
        .filter_map(|def| {
            let mut tool_tokens = tokens(def.name);
            tool_tokens.extend(tokens(def.description));
            let score = score(&query_tokens, &tool_tokens);
            (score > 0).then(|| (score, def.name.to_owned()))
        })
        .collect();
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, name)| name)
        .collect()
}

#[test]
fn equal_scores_keep_catalog_order() {
    let state = searchable();
    let got = page(&state, &json!({"query": "memory", "limit": 25}));
    assert_eq!(got, expected_ranking("memory", 25));
    assert!(got.len() > 1, "the fixture holds a tie: {got:?}");

    let mut alphabetical = got.clone();
    alphabetical.sort_unstable();
    assert_ne!(
        got, alphabetical,
        "the tie-break is catalog order, not the alphabet: {got:?}"
    );

    let group_order: Vec<String> = GROUP_TABLE
        .iter()
        .flat_map(|row| row.tools.iter().copied())
        .map(str::to_owned)
        .filter(|name| got.contains(name))
        .collect();
    assert_eq!(group_order.len(), got.len(), "every match has a group row");
    assert_ne!(
        got, group_order,
        "the tie-break is catalog order, not group-table order: {got:?}"
    );
}

#[test]
fn the_same_query_returns_the_same_order_twice() {
    let state = searchable();
    let args = json!({"query": "memory", "limit": 25});
    assert_eq!(page(&state, &args), page(&state, &args));
}

#[test]
fn an_oversized_limit_clamps_to_twenty_five() {
    let state = searchable();
    let query = "the";
    let matched = expected_ranking(query, usize::MAX);
    eprintln!("query `{query}` matches {} catalog tools", matched.len());
    assert!(
        matched.len() > 25,
        "the query must match more than one maximum page to observe the clamp: {} matches",
        matched.len()
    );

    let defaulted = page(&state, &json!({"query": query}));
    assert_eq!(
        defaulted.len(),
        10,
        "an absent limit returns the default page: {defaulted:?}"
    );

    let big = page(&state, &json!({"query": query, "limit": 1000}));
    let max = page(&state, &json!({"query": query, "limit": 25}));
    assert_eq!(big.len(), 25, "1000 clamps to 25 tools: {big:?}");
    assert_eq!(
        max.len(),
        25,
        "the maximum page is full, so the clamp is observable: {max:?}"
    );
    assert_eq!(big, max, "1000 and 25 return the same page in order");

    let query_tokens = tokens(query);
    for name in &big {
        let def = catalog_defs()
            .into_iter()
            .find(|def| def.name == name.as_str())
            .unwrap_or_else(|| panic!("a page names a catalog tool: {name}"));
        let mut tool_tokens = tokens(def.name);
        tool_tokens.extend(tokens(def.description));
        assert!(
            score(&query_tokens, &tool_tokens) > 0,
            "every returned tool matches the query: {name}"
        );
    }
}

#[test]
fn a_family_filter_returns_only_that_family_in_unfiltered_order() {
    let state = searchable();
    let all = page(&state, &json!({"query": "snapshot", "limit": 25}));
    let filtered = page(
        &state,
        &json!({"query": "snapshot", "family": "item-read", "limit": 25}),
    );
    assert!(!filtered.is_empty(), "the fixture family has a match");
    assert!(
        filtered
            .iter()
            .all(|name| group_of(name) == Some(ToolGroup::ItemRead)),
        "every row is in the requested family: {filtered:?}"
    );
    let expected: Vec<String> = all
        .into_iter()
        .filter(|name| group_of(name) == Some(ToolGroup::ItemRead))
        .collect();
    assert_eq!(filtered, expected, "the relative ranking is preserved");
    assert!(filtered.contains(&"get_snapshot".to_owned()));
}

#[test]
fn every_catalog_tool_is_reachable_by_its_own_name_and_description() {
    let state = searchable();
    let defs = catalog_defs();
    let total = defs.len();
    let mut missing = Vec::new();
    let mut worst_rank = 0usize;
    for def in &defs {
        let query = format!("{} {}", def.name, def.description);
        let got = page(&state, &json!({"query": query, "limit": 25}));
        match got.iter().position(|name| name == def.name) {
            Some(index) => worst_rank = worst_rank.max(index + 1),
            None => missing.push(def.name),
        }
    }
    eprintln!(
        "reachability: {}/{total} found at limit 25; worst rank {worst_rank}",
        total - missing.len()
    );
    assert!(
        missing.is_empty(),
        "tools missing from their own search page: {missing:?}"
    );
}
