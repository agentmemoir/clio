// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the `tool_search` contract: its
//! tokenizer, its raw overlap score, its argument validation, and the listing
//! entry and call envelope the discovery gate publishes.
//!
//! ## Owns
//! - Tokenizer cases for separators, Unicode lowercase, digits, and duplicates
//! - Score cases that pin the raw intersection and reject a bonus or a divisor
//! - Every refused argument shape, each mapped to `-32602`
//! - The four published listing keys and the success envelope shape
//! - The off and on behaviour of the `mcp.discovery` gate
//!
//! ## Does not own
//! - Rank order, ties, family order, and reachability
//!   (`catalog_search_ranking_tests`)
//! - Catalog membership or group data (`clio-types` tests)
//!
//! ## Boundary
//!
//! Changes to a search argument or to the published entry should normally
//! require changes here. Changes to ranking order should not.
//!
//! Keep this module focused on the contract and the gate.

use super::{NAME, call, envelope, listing_entry, score, tokens};
use crate::protocol::{ERR_INVALID_PARAMS, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// One token set from a written-out list, so a failure prints both sides.
fn set(words: &[&str]) -> BTreeSet<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// A server state whose configuration layer is exactly `config`.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// The advertised tool names for one state, in listing order.
fn listed(state: &McpState) -> Vec<String> {
    let value = McpHandler::tools_list_visible(state).expect("a known profile lists its tools");
    value["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
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

/// The RPC code a refused argument object returns.
fn refusal_code(args: &Value) -> i64 {
    call(args).expect_err("the arguments are refused").code
}

#[test]
fn underscores_hyphens_and_punctuation_split_tokens() {
    assert_eq!(
        tokens("get_snapshot, graph-query! (now)"),
        set(&["get", "snapshot", "graph", "query", "now"])
    );
}

#[test]
fn unicode_default_lowercase_forms_one_token_each() {
    assert_eq!(tokens("Ä"), set(&["ä"]));
    assert_eq!(tokens("ÄÖÜ"), set(&["äöü"]));
    // U+0130 lowercases to `i` plus a combining mark, and the mark splits away.
    assert_eq!(tokens("İ"), set(&["i"]));
}

#[test]
fn digits_stay_inside_a_token() {
    assert_eq!(tokens("tool2 v3"), set(&["tool2", "v3"]));
}

#[test]
fn empty_tokens_are_discarded_and_duplicates_collapse() {
    assert_eq!(tokens("  ),, get get GET  "), set(&["get"]));
    assert!(tokens(" ) , ; ").is_empty());
}

#[test]
fn score_is_the_raw_intersection_without_a_name_bonus() {
    let query = set(&["retrieve"]);
    let from_a_name = set(&["retrieve"]);
    let from_a_description = set(&["find", "retrieve", "items"]);
    assert_eq!(score(&query, &from_a_name), 1);
    assert_eq!(
        score(&query, &from_a_name),
        score(&query, &from_a_description),
        "a name match earns no extra point"
    );
}

#[test]
fn a_longer_tool_text_does_not_dilute_the_score() {
    let query = set(&["store"]);
    let short = set(&["store"]);
    let long = set(&[
        "store",
        "and",
        "retrieve",
        "a",
        "very",
        "long",
        "description",
        "with",
        "many",
        "words",
    ]);
    assert_eq!(
        score(&query, &short),
        score(&query, &long),
        "the score has no union denominator"
    );
}

#[test]
fn arguments_must_be_an_object() {
    assert_eq!(refusal_code(&json!([])), ERR_INVALID_PARAMS);
    assert_eq!(refusal_code(&json!("query")), ERR_INVALID_PARAMS);
    assert_eq!(refusal_code(&Value::Null), ERR_INVALID_PARAMS);
}

#[test]
fn query_is_required_and_must_hold_a_non_whitespace_character() {
    for args in [
        json!({}),
        json!({"query": ""}),
        json!({"query": "   \t"}),
        json!({"query": 5}),
        json!({"query": null}),
    ] {
        assert_eq!(refusal_code(&args), ERR_INVALID_PARAMS, "{args}");
    }
}

#[test]
fn limit_rejects_zero_negative_fraction_and_string() {
    for limit in [json!(0), json!(-1), json!(2.5), json!("5")] {
        let args = json!({"query": "store", "limit": limit});
        assert_eq!(refusal_code(&args), ERR_INVALID_PARAMS, "{args}");
    }
}

#[test]
fn an_unknown_family_is_refused() {
    let args = json!({"query": "store", "family": "no-such-family"});
    assert_eq!(refusal_code(&args), ERR_INVALID_PARAMS, "{args}");
    let non_string = json!({"query": "store", "family": 3});
    assert_eq!(
        refusal_code(&non_string),
        ERR_INVALID_PARAMS,
        "{non_string}"
    );
}

#[test]
fn an_unknown_argument_is_refused() {
    let args = json!({"query": "store", "bogus": true});
    assert_eq!(refusal_code(&args), ERR_INVALID_PARAMS, "{args}");
}

#[test]
fn the_listing_entry_publishes_only_the_four_contract_keys() {
    let entry = listing_entry();
    let object = entry.as_object().expect("an entry object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["description", "inputSchema", "name", "outputSchema"]);
    assert_eq!(entry["name"], json!(NAME));
    assert_eq!(entry["inputSchema"]["required"], json!(["query"]));
    assert!(entry["outputSchema"]["properties"]["tools"].is_object());
    assert!(entry.get("argumentRules").is_none());
    assert!(entry.get("examples").is_none());
}

#[test]
fn the_success_envelope_is_not_an_error_and_carries_the_page() {
    let page = json!({"tools": []});
    let result = envelope(page.clone());
    assert_eq!(result["isError"], json!(false));
    assert_eq!(result["structuredContent"], page);
    assert_eq!(result["content"][0]["type"], json!("text"));
    assert_eq!(result["content"][0]["text"], json!("ok"));
}

#[test]
fn discovery_off_hides_the_tool_and_refuses_the_call() {
    let state = state_with("{}");
    assert!(!listed(&state).contains(&NAME.to_owned()));
    let response = dispatched(&state, &json!({"query": "store"}));
    assert_eq!(response["error"]["code"].as_i64(), Some(ERR_INVALID_PARAMS));
    assert_eq!(
        response["error"]["message"].as_str(),
        Some("unknown tool `tool_search`")
    );
}

#[test]
fn discovery_on_lists_the_tool_and_answers_a_call() {
    let state = state_with(r#"{"mcp": {"discovery": true}}"#);
    assert!(listed(&state).contains(&NAME.to_owned()));
    let response = dispatched(&state, &json!({"query": "store", "limit": 1}));
    let tools = response["result"]["structuredContent"]["tools"]
        .as_array()
        .expect("a search page");
    assert_eq!(tools.len(), 1, "{response}");
}

#[test]
fn discovery_off_keeps_the_advertised_count_unchanged() {
    let absent = listed(&state_with("{}"));
    let off = listed(&state_with(r#"{"mcp": {"discovery": false}}"#));
    assert_eq!(absent, off, "the absent key and an explicit false agree");
    assert_eq!(
        absent.len(),
        clio_types::DEFAULT_PRESET.tools().len(),
        "the default listing keeps today's tool count"
    );
    let on = listed(&state_with(r#"{"mcp": {"discovery": true}}"#));
    assert_eq!(on.len(), absent.len() + 1);
    assert_eq!(on.iter().filter(|name| *name == NAME).count(), 1);
}
