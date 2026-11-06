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
//! - The minimal profile's listing as a literal, written-out tool set
//! - The `all` profile's listing as the full bound set
//! - A tool outside the selected profile being absent from the listing and still bound
//! - A configuration with no profile key, and two states with different profiles in one process
//! - An unknown profile name failing closed instead of listing a guessed preset
//! - An installed tool selection winning over the configured profile
//! - Each named preset, and a custom group list, listing exactly its own tools
//! - The default preset's listing when no selection and no profile key exist
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
use clio_types::{DEFAULT_PRESET, ToolGroup, ToolPreset};
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

/// The advertised tool names of a state carrying `groups` as the installed
/// selection — the seam a transport's `--tools` writes before it opens — over
/// the configuration document `config`.
fn listed_under(config: &str, groups: &[ToolGroup]) -> Vec<String> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    runtime.set_mcp_tool_groups(groups.to_vec());
    listed(&McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state"))
}

/// The retrieval group's tools, written out so a membership change fails these
/// tests instead of moving the expectation with the code.
const RETRIEVAL_TOOLS: [&str; 4] = ["associations", "compose_context", "intent_gate", "retrieve"];

/// The graph group's tools.
const GRAPH_TOOLS: [&str; 2] = ["graph_link", "graph_query"];

/// The item-read group's tools: the group a custom selection reaches through
/// the dependency closure rather than by naming it.
const ITEM_READ_TOOLS: [&str; 11] = [
    "audit_trail",
    "canonical_get",
    "consolidate",
    "get",
    "get_gist",
    "get_snapshot",
    "inspect",
    "maintenance_status",
    "scratchpad_read",
    "shared_retrieve",
    "summarize",
];

/// The persona group's tools.
const PERSONA_TOOLS: [&str; 3] = [
    "persona_get",
    "persona_observe_preference",
    "persona_put_stable",
];

/// The ops group's tools.
const OPS_TOOLS: [&str; 6] = ["diagnose", "doctor", "repair", "reindex", "stats", "verify"];

/// The hygiene group's tools.
const HYGIENE_TOOLS: [&str; 3] = ["hygiene_audit", "hygiene_clean", "hygiene_log_list"];

/// The retention group's tools.
const RETENTION_TOOLS: [&str; 2] = ["retention_profile_get", "retention_profile_set"];

/// `tools` owned and sorted, which is the shape the advertised names are compared
/// in.
fn sorted(tools: Vec<&str>) -> Vec<String> {
    let mut names: Vec<String> = tools.into_iter().map(str::to_owned).collect();
    names.sort_unstable();
    names
}

/// The listing the written-out group sets predict, sorted and de-duplicated the
/// way the advertised names are.
fn expected_of(groups: &[&[&str]]) -> Vec<String> {
    let mut names = sorted(
        groups
            .iter()
            .flat_map(|group| group.iter().copied())
            .collect(),
    );
    names.dedup();
    names
}

/// The minimal preset's expected listing, sorted.
fn expected_minimal() -> Vec<String> {
    expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS])
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
fn the_minimal_listing_is_exactly_its_written_out_tool_set() {
    let state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let names = listed(&state);
    assert_eq!(names, expected_minimal());
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
    assert_eq!(listed(&state), expected_minimal());
    let default_state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    assert_eq!(listed(&default_state), expected_minimal());
}

#[test]
fn a_state_opened_with_an_effective_document_reads_its_profile() {
    let all = McpState::open_with_effective(
        &McpOpenOptions::default(),
        &json!({"mcp": {"tool_profile": "all"}}),
    )
    .expect("state");
    let all_names = listed(&all);
    assert_eq!(all_names, full_catalog());
    assert!(all_names.contains(&"store".to_owned()), "{all_names:?}");

    let absent =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    assert_eq!(listed(&absent), expected_minimal());
}

#[test]
fn two_states_in_one_process_list_their_own_profiles() {
    let minimal = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);
    let all = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    let minimal_names = listed(&minimal);
    let all_names = listed(&all);
    assert_eq!(minimal_names, expected_minimal());
    assert_eq!(all_names, full_catalog());
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

/// A selection installed on the configuration layer — what a transport's
/// `--tools` does before it opens the state — wins over `mcp.tool_profile`, and
/// the listing is the selection's own tool set.
#[test]
fn an_installed_tool_selection_beats_the_configured_profile() {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(r#"{"mcp": {"tool_profile": "all"}}"#)
        .expect("a valid config layer");
    let groups = clio_types::custom(&["ops"]).expect("a known group");
    runtime.set_mcp_tool_groups(groups);
    let state = McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state");

    let names = listed(&state);
    assert_eq!(
        names,
        expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS, &ITEM_READ_TOOLS, &OPS_TOOLS]),
        "the listing is the written-out set for `ops` and what it requires"
    );
    assert_ne!(names, full_catalog(), "the configured `all` profile won");
    assert!(
        !names.contains(&"store".to_owned()),
        "a write tool outside the selection is advertised: {names:?}"
    );
}

/// Every preset name a transport's `--tools` accepts advertises exactly that
/// preset's tools. The written-out group data is the contract here: comparing
/// the listing against the preset's own derivation would restate the expression
/// the server evaluates instead of pinning what it produces.
#[test]
fn each_preset_selected_by_the_override_advertises_that_presets_tool_set() {
    for (preset, written_out) in [
        (
            ToolPreset::Minimal,
            expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS]),
        ),
        (
            ToolPreset::Coding,
            expected_of(&[&RETRIEVAL_TOOLS, &ITEM_READ_TOOLS, &PERSONA_TOOLS]),
        ),
        (
            ToolPreset::Ops,
            expected_of(&[
                &ITEM_READ_TOOLS,
                &OPS_TOOLS,
                &HYGIENE_TOOLS,
                &RETENTION_TOOLS,
            ]),
        ),
        (ToolPreset::All, full_catalog()),
    ] {
        let name = preset.as_str();
        let advertised = listed_under(r#"{"mcp": {"tool_profile": "minimal"}}"#, &preset.resolve());
        assert_eq!(
            advertised, written_out,
            "`--tools {name}` advertised {advertised:?}"
        );
    }
}

/// A custom group list is accepted, and the listing carries the groups the
/// dependency closure adds: `item-read` arrives with `ops` although the value
/// never names it, and the closure stops at what `ops` requires.
#[test]
fn a_custom_selection_advertises_the_groups_its_closure_adds() {
    let groups = clio_types::custom(&["ops"]).expect("a known group");
    assert!(
        ToolGroup::Ops.requires().contains(&ToolGroup::ItemRead),
        "premise: `ops` requires the group the selection never names"
    );
    assert!(
        groups.contains(&ToolGroup::ItemRead),
        "the closure resolves `item-read` from `ops` alone"
    );
    let advertised = listed_under("{}", &groups);
    assert_eq!(
        advertised,
        expected_of(&[&RETRIEVAL_TOOLS, &ITEM_READ_TOOLS, &GRAPH_TOOLS, &OPS_TOOLS]),
        "the listing is the minimal set plus `ops` and everything `ops` requires"
    );
    for tool in ["canonical_get", "inspect", "shared_retrieve"] {
        assert!(
            advertised.contains(&tool.to_owned()),
            "`{tool}` belongs to the closure-added `item-read` group: {advertised:?}"
        );
    }
    assert!(
        !advertised.contains(&"store".to_owned()),
        "the closure did not reach `item-write`: {advertised:?}"
    );
}

/// With no selection installed and no `mcp.tool_profile` written, the server
/// advertises the default preset's set and nothing wider.
#[test]
fn no_selection_and_no_profile_setting_lists_the_default_preset_set() {
    let advertised = listed(&state_with("{}"));
    assert_eq!(advertised, expected_minimal());
    assert_eq!(advertised, sorted(DEFAULT_PRESET.tools()));
    assert!(
        !advertised.contains(&"get".to_owned()),
        "the default preset does not reach `item-read`: {advertised:?}"
    );
}
