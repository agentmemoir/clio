// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cursor tests over the production view
//! seam: a real change wakes one listener and refuses the old cursor, an equal
//! replacement changes nothing, and a profile change that keeps the same tools
//! still refuses the cursor.
//!
//! ## Owns
//! - The listener wait through the real condition-variable path
//! - The cursor refusal after a replacement
//! - The digest-bound cursor despite an unchanged tool list
//!
//! ## Does not own
//! - The listen envelopes or their stream (see `tool_listen`)
//! - The change signal (see `tool_signals`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::protocol::ERR_INVALID_PARAMS;
use crate::stateless_fixtures::state;
use serde_json::json;
use std::time::Duration;

#[test]
fn a_real_view_change_wakes_one_listener_and_refuses_the_old_cursor() {
    let state = state();
    let page = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 1 } })),
    )
    .expect("the first page is served");
    let cursor = page["nextCursor"]
        .as_str()
        .expect("a small page carries a cursor")
        .to_owned();
    let (_, subscriber) = state.tool_signals.register().expect("register");
    assert_eq!(subscriber.observed(), 0, "the listener starts at the seed");

    {
        let mut config = state.config.lock().expect("config mutex");
        config.set_mcp_tool_groups(clio_types::ToolGroup::ALL.to_vec());
    }
    assert!(state.replace_tool_view(), "the changed view is installed");
    assert_eq!(
        subscriber.wait_from(0, Duration::from_millis(50)),
        1,
        "one change wakes the listener once"
    );

    let error =
        crate::protocol::McpHandler::tools_list_page(&state, Some(&json!({ "cursor": cursor })))
            .expect_err("a cursor from the old view is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}

#[test]
fn an_equal_replacement_is_a_no_op_and_the_same_cursor_pages_on() {
    let state = state();
    let page = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 1 } })),
    )
    .expect("the first page is served");
    let cursor = page["nextCursor"]
        .as_str()
        .expect("a small page carries a cursor")
        .to_owned();
    let (_, subscriber) = state.tool_signals.register().expect("register");

    assert!(
        !state.replace_tool_view(),
        "an equal snapshot is not a change"
    );
    assert_eq!(
        subscriber.wait_from(0, Duration::from_millis(50)),
        0,
        "the change count does not move"
    );
    let next =
        crate::protocol::McpHandler::tools_list_page(&state, Some(&json!({ "cursor": cursor })))
            .expect("the old cursor still pages the same view");
    assert_eq!(
        next["tools"].as_array().expect("tools").len(),
        1,
        "the same next page"
    );
}

/// The cursor binds to the view digest, not to the visible tool list: a
/// profile change that leaves the same tools installed still refuses it.
#[test]
fn a_profile_change_that_keeps_the_same_tools_still_refuses_the_cursor() {
    let state = state();
    {
        let mut config = state.config.lock().expect("config mutex");
        config.set_mcp_tool_groups(clio_types::custom(&["ops"]).expect("a known group"));
    }
    let page = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 1 } })),
    )
    .expect("the first page is served");
    let cursor = page["nextCursor"]
        .as_str()
        .expect("a small page carries a cursor")
        .to_owned();

    let before = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 100 } })),
    )
    .expect("the whole visible list");
    let before_names: Vec<serde_json::Value> = before["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].clone())
        .collect();

    {
        let mut config = state.config.lock().expect("config mutex");
        config
            .load_file_json(r#"{"mcp": {"tool_profile": "all"}}"#)
            .expect("the profile layer parses");
    }
    assert!(
        state.replace_tool_view(),
        "the profile change installs a new view"
    );
    let after = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 100 } })),
    )
    .expect("the whole visible list after the change");
    assert_eq!(
        before_names,
        after["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>(),
        "the visible tools did not change"
    );

    let error =
        crate::protocol::McpHandler::tools_list_page(&state, Some(&json!({ "cursor": cursor })))
            .expect_err("a cursor from the old digest is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}
