// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the two `clio mcp` transports wire their `--tools` and
//! `--discovery` selections: a value no preset and no group list can honour
//! stops the start with a usage error, a valid value reaches the served
//! `tools/list`, and both `--help` screens state the default that applies when
//! no flag is given.
//!
//! ## Owns
//! - The usage refusal (exit 2) reaching both transport entry points, naming the
//!   value it refused
//! - The selection a valid value names arriving in the listing the transport
//!   actually serves, as a written-out tool set
//! - A default first page carrying the three discovery tools for the widest
//!   preset and a large custom list
//! - The `--discovery` switch turning the three discovery tools on, and off
//!   when the configuration document sets them on
//! - An arbitrary client `_meta` key leaving the listing unchanged
//! - The `--tools` block of both help screens
//!
//! ## Does not own
//! - What the flags accept and the groups they yield (`mcp_tools_flag_tests`,
//!   `mcp_discovery_flag_tests`)
//! - The transports themselves (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a transport consumes a selection should normally require
//! changes here. Changes to the accepted values should not.
//!
//! Keep this module focused on the transports' selection wiring.

use super::tests::args_of;
use super::*;

/// A `--tools` value no preset and no group list can honour is refused before
/// any transport opens a store, so no server starts on a value it cannot serve
/// and none advertises a set it guessed.
#[test]
fn a_bad_tools_value_stops_both_transports_with_a_usage_error() {
    for value in ["mnimal", "custom:ops,bogus", "custom:"] {
        for transport in ["stdio", "http"] {
            let code = mcp(args_of(&[transport, "--tools", value]));
            assert_eq!(code, ExitCode::from(2), "{transport} served `{value}`");
        }
    }
}

/// The refusal a bad value raises names the value that was written, so a caller
/// can see which part of it this build does not know, and yields no groups a
/// state could be opened with.
#[test]
fn a_bad_tools_value_is_refused_naming_it_and_yields_no_groups() {
    for (value, offender) in [
        ("mnimal", "mnimal"),
        ("custom:ops,bogus", "bogus"),
        ("custom:", "custom:"),
    ] {
        let error = crate::mcp_tools_flag::selection(&crate::cli_flags_test_support::flags_of(&[
            "--tools", value,
        ]))
        .expect_err("the value names no set this build can build");
        assert_eq!(error.exit_code(), ExitCode::from(2), "`{value}`");
        assert!(error.message().contains(offender), "{}", error.message());
    }
}

/// Both `--help` screens state what `--tools` takes and what it does not: the
/// preset names, the `custom:` form, and the config setting that decides when
/// no flag is given, with `minimal` as the fallback.
#[test]
fn both_transport_help_screens_state_the_tools_default() {
    for help in [mcp_stdio_help(), mcp_http_help()] {
        assert!(help.contains("--tools PRESET"), "{help}");
        for preset in ["`minimal`", "`coding`", "`ops`", "`all`"] {
            assert!(help.contains(preset), "`{preset}` is not offered: {help}");
        }
        assert!(help.contains("custom:GROUP[,GROUP]"), "{help}");
        assert!(help.contains("mcp.tool_profile"), "{help}");
        assert!(help.contains("`minimal` preset"), "{help}");
    }
}

// ------------------------------------------------------------ served listing

/// The retrieval group's tools, written out so a membership change fails this
/// module instead of moving the expectation with the code.
const RETRIEVAL_TOOLS: [&str; 4] = ["associations", "compose_context", "intent_gate", "retrieve"];

/// The graph group's tools.
const GRAPH_TOOLS: [&str; 2] = ["graph_link", "graph_query"];

/// The item-read group's tools: the group a `custom:` value reaches through the
/// dependency closure rather than by naming it.
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

/// The written-out groups' tools, flattened, sorted, and de-duplicated the way
/// the advertised names are compared.
fn expected_of(groups: &[&[&str]]) -> Vec<String> {
    let mut names: Vec<String> = groups
        .iter()
        .flat_map(|group| group.iter().copied())
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The groups one `--tools` value names, decoded by the same reader the
/// transports use.
fn selection_of(value: &str) -> Vec<clio_types::ToolGroup> {
    crate::mcp_tools_flag::selection(&crate::cli_flags_test_support::flags_of(&[
        "--tools", value,
    ]))
    .expect("a value this build honours")
    .expect("the flag was given")
}

/// The three discovery tools, sorted.
const DISCOVERY_TOOLS: [&str; 3] = ["tool_describe", "tool_invoke", "tool_search"];

/// One whole `tools/list` request, so a test names the exact request it sends.
const TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;

/// The raw response bytes one `tools/list` request gets over the stdio serve
/// path, with `config` as the whole configuration layer and `groups` and
/// `discovery` as the transport's selections.
///
/// The runtime reads no process environment, so the answer cannot depend on the
/// host's deployment config or on any other host setting.
fn served_bytes_under(
    config: &str,
    groups: Option<&[clio_types::ToolGroup]>,
    discovery: Option<bool>,
    request: &str,
) -> Vec<u8> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &clio_mcp::runtime::McpOpenOptions::default(),
            runtime,
            groups,
            discovery,
            std::io::Cursor::new(format!("{request}\n").into_bytes()),
            &mut out,
        ),
        ExitCode::SUCCESS,
        "the session did not serve"
    );
    out
}

/// The raw response bytes one `tools/list` request gets over the stdio serve
/// path under an empty configuration layer.
fn served_bytes(
    groups: Option<&[clio_types::ToolGroup]>,
    discovery: Option<bool>,
    request: &str,
) -> Vec<u8> {
    served_bytes_under("{}", groups, discovery, request)
}

/// The advertised tool names in one served response, in the order served.
fn ordered_names_in(bytes: &[u8]) -> Vec<String> {
    let response: serde_json::Value =
        serde_json::from_slice(bytes).expect("one response line per request");
    response["result"]["tools"]
        .as_array()
        .expect("a served tools/list result carries a tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The advertised tool names in one served response, sorted.
fn names_in(bytes: &[u8]) -> Vec<String> {
    let mut names = ordered_names_in(bytes);
    names.sort_unstable();
    names
}

/// The tool names one `tools/list` answered over the stdio serve path with
/// `groups` as the transport's selection, sorted.
///
/// This is the whole flag-to-listing chain in one call: the value is decoded by
/// the transport's reader, served by [`mcp_stdio_serve`], installed on the
/// runtime opened for the session, and read back by the handler. The target is
/// in-memory and the selection outranks the configured document, so the answer
/// depends on the flag alone — not on the host's configuration, ports, or
/// database.
fn served_listing(groups: &[clio_types::ToolGroup]) -> Vec<String> {
    names_in(&served_bytes(Some(groups), None, TOOLS_LIST))
}

/// The advertised tool names of a state opened over `config` with `discovery`
/// installed — the seam a transport's `--discovery` writes before it opens.
fn names_under(config: &str, discovery: Option<bool>) -> Vec<String> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    runtime.set_mcp_discovery(discovery);
    let state = clio_mcp::runtime::McpState::open_with_runtime(
        &clio_mcp::runtime::McpOpenOptions::default(),
        runtime,
    )
    .expect("state");
    let response = clio_mcp::protocol::McpHandler::new(state)
        .handle_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list"
        }))
        .expect("a request with an id has a response");
    let mut names: Vec<String> = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// The smallest preset's `--tools` value reaches the served listing, which
/// advertises that preset's written-out tool set and nothing wider.
#[test]
fn the_smallest_preset_flag_reaches_the_served_listing() {
    let advertised = served_listing(&selection_of("minimal"));
    assert_eq!(
        advertised,
        expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS]),
        "`--tools minimal` advertised {advertised:?}"
    );
    assert!(
        !advertised.contains(&"store".to_owned()),
        "a write tool outside the smallest preset is advertised: {advertised:?}"
    );
}

/// A wider preset's `--tools` value reaches the served listing. This is the case
/// a dropped install cannot fake: with the value ignored, the configured or
/// default preset answers instead, and that answer is the smallest set.
#[test]
fn a_wider_preset_flag_reaches_the_served_listing() {
    let advertised = served_listing(&selection_of("coding"));
    assert_eq!(
        advertised,
        expected_of(&[&RETRIEVAL_TOOLS, &ITEM_READ_TOOLS, &PERSONA_TOOLS]),
        "`--tools coding` advertised {advertised:?}"
    );
    assert_ne!(
        advertised,
        expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS]),
        "the default preset answered instead of the flag"
    );
}

/// A `custom:` value reaches the served listing carrying the groups its
/// dependency closure adds, although the value never names them.
#[test]
fn a_custom_flag_reaches_the_served_listing_through_its_closure() {
    let advertised = served_listing(&selection_of("custom:ops"));
    assert_eq!(
        advertised,
        expected_of(&[&RETRIEVAL_TOOLS, &GRAPH_TOOLS, &ITEM_READ_TOOLS, &OPS_TOOLS]),
        "`--tools custom:ops` advertised {advertised:?}"
    );
    for tool in ["canonical_get", "inspect", "summarize"] {
        assert!(
            advertised.contains(&tool.to_owned()),
            "`{tool}` arrives with the `item-read` group `ops` requires: {advertised:?}"
        );
    }
}

// -------------------------------------------------------- discovery switch

/// No `--discovery` leaves the configuration document in charge, and its
/// default keeps the three discovery tools off.
#[test]
fn no_flag_advertises_no_discovery_tool() {
    let advertised = names_in(&served_bytes(None, None, TOOLS_LIST));
    for tool in DISCOVERY_TOOLS {
        assert!(
            !advertised.contains(&tool.to_owned()),
            "`{tool}` is advertised without `--discovery`: {advertised:?}"
        );
    }
}

/// `--discovery on` reaches the served listing and advertises all three
/// discovery tools.
#[test]
fn discovery_on_serves_the_three_tools() {
    let advertised = names_in(&served_bytes(None, Some(true), TOOLS_LIST));
    for tool in DISCOVERY_TOOLS {
        assert!(
            advertised.contains(&tool.to_owned()),
            "`--discovery on` must advertise `{tool}`: {advertised:?}"
        );
    }
}

/// The configuration document turns the three tools on, and `--discovery off`
/// hides them anyway: the flag wins over `mcp.discovery` on its own axis.
#[test]
fn discovery_off_hides_them_despite_the_config() {
    let config = r#"{"mcp": {"discovery": true}}"#;
    let configured = names_under(config, None);
    for tool in DISCOVERY_TOOLS {
        assert!(
            configured.contains(&tool.to_owned()),
            "the configuration document turns `{tool}` on: {configured:?}"
        );
    }
    let off = crate::mcp_discovery_flag::selection(&crate::cli_flags_test_support::flags_of(&[
        "--discovery",
        "off",
    ]))
    .expect("`off` is accepted");
    assert_eq!(off, Some(false));
    let hidden = names_under(config, off);
    for tool in DISCOVERY_TOOLS {
        assert!(
            !hidden.contains(&tool.to_owned()),
            "`--discovery off` must hide `{tool}`: {hidden:?}"
        );
    }
}

/// Two `tools/list` requests that differ only in an arbitrary `params._meta`
/// key answer identical bytes, so the listing reads and claims no client
/// deferral signal.
#[test]
fn client_meta_never_changes_the_listing() {
    let plain = served_bytes(None, Some(true), TOOLS_LIST);
    let with_meta = served_bytes(
        None,
        Some(true),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"com.example/clientDeferLoading":true}}}"#,
    );
    assert_eq!(
        plain, with_meta,
        "a client `_meta` key must not change the listing or claim a deferral"
    );
}

/// A `--discovery` value this build cannot honour stops both transports with
/// exit 2 before either opens a store, including the `true` a bare flag
/// produces.
#[test]
fn a_bad_discovery_value_stops_both_transports_with_exit_2() {
    for value in ["true", "yes", "ON", "1"] {
        for transport in ["stdio", "http"] {
            assert_eq!(
                mcp(args_of(&[transport, "--discovery", value])),
                ExitCode::from(2),
                "{transport} served `{value}`"
            );
        }
    }
    for transport in ["stdio", "http"] {
        assert_eq!(
            mcp(args_of(&[transport, "--discovery"])),
            ExitCode::from(2),
            "{transport} defaulted a bare `--discovery`"
        );
    }
}

// The page-order and switch-precedence cases live in a child module: they need
// the served helpers above, and moving them out is what leaves this file room
// for the transport-wiring cases.
#[cfg(test)]
#[path = "mcp_cli_discovery_page_tests.rs"]
mod discovery_page_tests;
