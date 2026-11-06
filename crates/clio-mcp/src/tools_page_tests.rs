// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `tools/list` pagination: the
//! default page, the clamp, the single-page and multi-page shapes, the cursor
//! refusals, and the repeatability of one page.
//!
//! ## Owns
//! - The default page size and the clamp on a request above the maximum
//! - A page that ends the view carrying no `nextCursor`
//! - A full walk of the `all` profile visiting every tool once, in order
//! - Byte-identical results for identical requests
//! - The refusals for a size disagreement, a malformed cursor, and an unknown
//!   cursor format version
//! - A cursor refused when another preset governs the view, and two presets
//!   minting two cursors over one tool set
//! - A cursor offset at or past the end of the view, and the empty view's
//!   initial page
//!
//! ## Does not own
//! - The page-size constants and the cursor code itself (`tools_page`)
//! - The visible view and the profile behind it (`tool_profile_tests`)
//! - The JSON-RPC error code set (`protocol`)
//!
//! ## Boundary
//!
//! Changes to how a listing is sliced, or to the cursor it carries, should
//! normally require changes here. Changes to profile membership or to the
//! catalog data should not.
//!
//! Keep this module focused on the paging behaviour of one listing.

use crate::protocol::{ERR_INVALID_PARAMS, McpHandler, RpcError};
use crate::runtime::{McpOpenOptions, McpState};
use crate::tools_page::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, PAGE_SIZE_KEY, ToolView, page};
use base64::Engine;
use clio_types::ToolPreset;
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

/// The `all` profile, which lists every bound tool.
fn all_state() -> Arc<McpState> {
    state_with(r#"{"mcp": {"tool_profile": "all"}}"#)
}

/// Send one `tools/list` and return the whole response.
fn request(state: &Arc<McpState>, params: &Value) -> Value {
    McpHandler::new(Arc::clone(state))
        .handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": params
        }))
        .expect("a request with an id has a response")
}

/// The tool names one result value carries, in the order it carries them.
fn tool_names(result: &Value) -> Vec<String> {
    result["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The tool names one response carries, in the order it carries them.
fn names(response: &Value) -> Vec<String> {
    tool_names(&response["result"])
}

/// A synthetic catalog of `count` tools, so a page boundary is exercised
/// without depending on how many tools this build publishes.
fn synthetic(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({ "name": format!("tool_{index}") }))
        .collect()
}

/// The identity of one synthetic view under `preset`.
fn view(preset: ToolPreset) -> ToolView {
    ToolView {
        preset: Some(preset),
        groups: preset.resolve(),
        revision: "2025-11-25".to_owned(),
    }
}

/// One page of a synthetic catalog under `preset`.
fn preset_page(tools: &[Value], preset: ToolPreset, params: &Value) -> Result<Value, RpcError> {
    page(tools, &view(preset), Some(params))
}

/// One page of a synthetic catalog under the `all` preset.
fn synthetic_page(tools: &[Value], params: &Value) -> Result<Value, RpcError> {
    preset_page(tools, ToolPreset::All, params)
}

/// The cursor one response carries, if any.
fn cursor(response: &Value) -> Option<&str> {
    response["nextCursor"].as_str()
}

/// Encode one cursor tuple the way the module does, so a test can forge a
/// cursor the module must refuse.
fn encode_raw(fields: &Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(fields).expect("a cursor tuple serializes"))
}

/// The cursor of a first page, for the tests that continue a walk.
fn first_cursor(tools: &[Value], size: usize) -> String {
    let response = synthetic_page(tools, &json!({ "_meta": { PAGE_SIZE_KEY: size } }))
        .expect("a valid first page");
    cursor(&response)
        .expect("a first page with more tools carries a cursor")
        .to_owned()
}

/// Rewrite the offset field of a real cursor, so a test can aim it at a view
/// position the server never mints.
fn with_offset(encoded: &str, offset: usize) -> String {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .expect("a real cursor is valid base64url");
    let mut fields: Value = serde_json::from_slice(&bytes).expect("a real cursor is a JSON tuple");
    fields[3] = json!(offset);
    encode_raw(&fields)
}

#[test]
fn the_default_page_carries_25_tools() {
    let tools = synthetic(60);
    let response = synthetic_page(&tools, &json!({})).expect("a valid page");
    assert_eq!(
        response["tools"].as_array().expect("tools").len(),
        DEFAULT_PAGE_SIZE
    );
    assert_eq!(DEFAULT_PAGE_SIZE, 25);
    assert!(
        cursor(&response).is_some(),
        "a page that stops early continues"
    );
}

#[test]
fn a_page_size_above_the_maximum_is_clamped() {
    let tools = synthetic(150);
    let response =
        synthetic_page(&tools, &json!({ "_meta": { PAGE_SIZE_KEY: 1000 } })).expect("a valid page");
    assert_eq!(
        response["tools"].as_array().expect("tools").len(),
        MAX_PAGE_SIZE
    );
    assert_eq!(MAX_PAGE_SIZE, 100);
    assert!(
        cursor(&response).is_some(),
        "the clamp still leaves a next page"
    );
}

#[test]
fn a_page_that_ends_the_view_omits_the_cursor() {
    let tools = synthetic(10);
    let response = synthetic_page(&tools, &json!({})).expect("a valid page");
    assert_eq!(response["tools"].as_array().expect("tools").len(), 10);
    assert!(cursor(&response).is_none(), "no next page means no cursor");
}

#[test]
fn a_full_walk_returns_every_tool_once_in_order() {
    let state = all_state();
    let expected = tool_names(&McpHandler::tools_list());
    assert!(
        expected.len() > DEFAULT_PAGE_SIZE,
        "the premise is a view that needs more than one page"
    );

    let mut walked: Vec<String> = Vec::new();
    let mut params = json!({ "_meta": { PAGE_SIZE_KEY: 10 } });
    loop {
        let response = request(&state, &params);
        assert!(response.get("error").is_none(), "{response}");
        walked.extend(names(&response));
        match response["result"]["nextCursor"].as_str() {
            Some(next) => params = json!({ "cursor": next }),
            None => break,
        }
    }
    assert_walk(&walked, &expected);
}

/// One assertion that the walk visited the whole view once, in order.
fn assert_walk(walked: &[String], expected: &[String]) {
    assert_eq!(walked.len(), expected.len(), "every tool appears once");
    assert_eq!(walked, expected, "the walk keeps catalog order");
    let mut sorted = walked.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), walked.len(), "no tool appears twice");
}

#[test]
fn identical_requests_produce_identical_bytes() {
    let state = all_state();
    let params = json!({ "_meta": { PAGE_SIZE_KEY: 7 } });
    let first = request(&state, &params);
    let second = request(&state, &params);
    assert_eq!(
        serde_json::to_vec(&first["result"]).expect("a result serializes"),
        serde_json::to_vec(&second["result"]).expect("a result serializes"),
        "an unchanged view answers the same bytes, cursor included"
    );

    let next = json!({ "cursor": cursor(&first["result"]).expect("a next page") });
    let third = request(&state, &next);
    let fourth = request(&state, &next);
    assert_eq!(
        serde_json::to_vec(&third["result"]).expect("a result serializes"),
        serde_json::to_vec(&fourth["result"]).expect("a result serializes"),
        "a second page is repeatable too"
    );
}

#[test]
fn a_page_size_that_disagrees_with_the_cursor_is_refused() {
    let tools = synthetic(60);
    let cursor = first_cursor(&tools, 10);
    let error = synthetic_page(
        &tools,
        &json!({ "cursor": cursor, "_meta": { PAGE_SIZE_KEY: 20 } }),
    )
    .expect_err("a size that contradicts the cursor is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(error.message.contains(PAGE_SIZE_KEY), "{}", error.message);
}

#[test]
fn a_malformed_cursor_is_refused() {
    let tools = synthetic(60);
    for cursor in [
        "not base64url!!".to_owned(),
        "Zm9v".to_owned(),
        encode_raw(&json!([1, "digest"])),
    ] {
        let error = synthetic_page(&tools, &json!({ "cursor": cursor }))
            .expect_err("a malformed cursor is refused");
        assert_eq!(error.code, ERR_INVALID_PARAMS, "{}", error.message);
    }
}

#[test]
fn a_cursor_with_an_unknown_format_version_is_refused() {
    let tools = synthetic(60);
    let forged = encode_raw(&json!([2, "fnv1a64:0000000000000000", 10, 10]));
    let error = synthetic_page(&tools, &json!({ "cursor": forged }))
        .expect_err("a cursor version this build does not write is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(error.message.contains("version"), "{}", error.message);
}

/// A cursor minted under one preset is refused while another preset governs
/// the view, because the digest binds the cursor to one view.
#[test]
fn a_cursor_from_another_preset_is_refused() {
    let tools = synthetic(60);
    let cursor = first_cursor(&tools, 10);
    let error = preset_page(&tools, ToolPreset::Minimal, &json!({ "cursor": cursor }))
        .expect_err("a cursor from another view is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}

/// One tool set under two presets mints two different first-page cursors, so a
/// cursor always names the preset that produced it.
#[test]
fn the_first_page_cursor_differs_between_presets() {
    let tools = synthetic(60);
    let all = preset_page(&tools, ToolPreset::All, &json!({})).expect("a valid page");
    let minimal = preset_page(&tools, ToolPreset::Minimal, &json!({})).expect("a valid page");
    assert_ne!(
        cursor(&all).expect("the `all` page continues"),
        cursor(&minimal).expect("the `minimal` page continues"),
        "the cursor is bound to the preset that minted it"
    );
}

#[test]
fn a_cursor_offset_at_the_end_of_the_view_is_refused() {
    let tools = synthetic(60);
    let encoded = first_cursor(&tools, 10);
    let error = synthetic_page(
        &tools,
        &json!({ "cursor": with_offset(&encoded, tools.len()) }),
    )
    .expect_err("an offset at the end of the view is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(error.message.contains("past the end"), "{}", error.message);
}

#[test]
fn a_cursor_offset_past_the_end_of_the_view_is_refused() {
    let tools = synthetic(60);
    let encoded = first_cursor(&tools, 10);
    let error = synthetic_page(
        &tools,
        &json!({ "cursor": with_offset(&encoded, tools.len() + 1) }),
    )
    .expect_err("an offset past the end of the view is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
}

#[test]
fn an_empty_view_answers_an_initial_request_with_an_empty_terminal_page() {
    let response = synthetic_page(&[], &json!({})).expect("an empty view has a page");
    assert_eq!(response, json!({ "tools": [] }));
    assert!(
        cursor(&response).is_none(),
        "an empty view has no next page"
    );
}

#[test]
fn a_changed_catalog_invalidates_a_cursor() {
    let tools = synthetic(60);
    let encoded = first_cursor(&tools, 10);
    let mut changed = tools.clone();
    changed[0] = json!({ "name": "renamed_tool" });
    let error = synthetic_page(&changed, &json!({ "cursor": encoded }))
        .expect_err("a cursor from another catalog is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}

#[test]
fn a_changed_revision_invalidates_a_cursor() {
    let tools = synthetic(60);
    let encoded = first_cursor(&tools, 10);
    let mut changed = view(ToolPreset::All);
    changed.revision = "2026-01-01".to_owned();
    let error = page(&tools, &changed, Some(&json!({ "cursor": encoded })))
        .expect_err("a cursor from another revision is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}

#[test]
fn a_page_size_below_one_or_not_an_integer_is_refused() {
    let tools = synthetic(60);
    for size in [json!(0), json!(-1), json!(1.5), json!("ten")] {
        let error = synthetic_page(&tools, &json!({ "_meta": { PAGE_SIZE_KEY: size } }))
            .expect_err("a page size outside the accepted range is refused");
        assert_eq!(error.code, ERR_INVALID_PARAMS, "{}", error.message);
        assert!(error.message.contains(PAGE_SIZE_KEY), "{}", error.message);
    }
}

/// A document preset this build does not know must not fail `tools/list` when
/// an installed selection governs the view; the listing is that selection.
#[test]
fn an_installed_selection_serves_a_listing_over_an_unknown_document_preset() {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(r#"{"mcp": {"tool_profile": "mnimal"}}"#)
        .expect("a valid config layer");
    runtime.set_mcp_tool_groups(clio_types::custom(&["ops"]).expect("a known group"));
    let state = McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state");

    let response = request(&state, &json!({ "_meta": { PAGE_SIZE_KEY: 100 } }));
    assert!(response.get("error").is_none(), "{response}");
    let names = names(&response);
    assert!(!names.is_empty(), "the installed selection lists tools");
    assert!(
        !names.contains(&"store".to_owned()),
        "a write tool outside the selection is advertised: {names:?}"
    );
}
