// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the advertised `tools/list`
//! result to the tool profile the configuration selects, while the bound
//! catalog keeps answering dispatch and the argument contract.
//!
//! ## Owns
//! - The minimal profile's listing as its own tool set intersected with the bound set
//! - A tool outside the selected profile being absent from the listing and still bound
//! - A configuration with no profile key, and two states with different profiles in one process
//! - An unknown profile name failing closed instead of listing a guessed preset
//!
//! ## Does not own
//! - The protocol envelope and error mapping (`protocol_tests`)
//! - Group membership, presets, and closures (`clio-types` tests)
//!
//! ## Boundary
//!
//! Changes to what `tools/list` advertises should normally require changes here.
//! Changes to the catalog data or to transport framing should not.
//!
//! Keep this module focused on the visible listing and the profile behind it.

use crate::bound_tools;
use crate::protocol::{ERR_INTERNAL, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::ToolPreset;
use serde_json::json;
use std::sync::Arc;

/// A server state whose configuration layer is exactly `config`.
///
/// The runtime reads no process environment, so the listing depends on the given
/// document alone.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// The advertised tool names, sorted so the comparison never depends on order.
fn listed(state: &McpState) -> Vec<String> {
    let value = McpHandler::tools_list_visible(state).expect("a known profile lists its tools");
    let mut names: Vec<String> = value["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// A preset's own tool set intersected with what this build binds, sorted.
fn expected(preset: ToolPreset) -> Vec<String> {
    let bound = bound_tools();
    let mut names: Vec<String> = preset
        .tools()
        .into_iter()
        .filter(|tool| bound.contains(tool))
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names
}

/// The full catalogue the dispatcher and the argument contract read, sorted.
fn full_catalog() -> Vec<String> {
    let mut names: Vec<String> = McpHandler::tools_list()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

#[test]
fn the_minimal_listing_is_its_profile_intersected_with_the_bound_set() {
    let state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let names = listed(&state);
    assert_eq!(names, expected(ToolPreset::Minimal));
    assert!(names.contains(&"retrieve".to_owned()), "{names:?}");
    assert!(!names.is_empty(), "the minimal profile lists its own tools");
}

#[test]
fn a_tool_outside_the_minimal_profile_is_not_advertised_but_stays_bound() {
    let state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    assert!(
        !listed(&state).contains(&"store".to_owned()),
        "a write tool is outside the minimal profile"
    );
    assert!(
        bound_tools().contains(&"store"),
        "this build still binds the tool the profile hides"
    );
    assert!(
        full_catalog().contains(&"store".to_owned()),
        "the full catalogue still answers dispatch and argument validation"
    );
}

#[test]
fn a_configuration_without_the_key_lists_the_minimal_profile() {
    let state = state_with("{}");
    assert_eq!(listed(&state), expected(ToolPreset::Minimal));
    let default_state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    assert_eq!(listed(&default_state), expected(ToolPreset::Minimal));
}

#[test]
fn two_states_in_one_process_list_their_own_profiles() {
    let minimal = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let all = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    let minimal_names = listed(&minimal);
    let all_names = listed(&all);
    assert_eq!(minimal_names, expected(ToolPreset::Minimal));
    assert_eq!(all_names, expected(ToolPreset::All));
    assert_ne!(minimal_names, all_names);
    assert!(
        all_names.len() > minimal_names.len(),
        "the wider profile lists more tools"
    );
    assert!(all_names.contains(&"store".to_owned()));
}

#[test]
fn an_unknown_profile_fails_closed() {
    let state = state_with(r#"{"mcp": {"tool_profile": "mnimal"}}"#);
    let error =
        McpHandler::tools_list_visible(&state).expect_err("an unknown profile lists nothing");
    assert_eq!(error.code, ERR_INTERNAL);
    assert!(error.message.contains("mnimal"), "{}", error.message);
    assert!(error.message.contains("minimal"), "{}", error.message);
}
