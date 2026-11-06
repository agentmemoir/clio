// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the two `clio mcp` transports wire their `--tools`
//! selection: a value no preset and no group list can honour stops the start
//! with a usage error, a valid value reaches the served `tools/list`, and both
//! `--help` screens state the default that applies when no flag is given.
//!
//! ## Owns
//! - The usage refusal (exit 2) reaching both transport entry points, naming the
//!   value it refused
//! - The selection a valid value names arriving in the listing the transport
//!   actually serves, as a written-out tool set
//! - The `--tools` block of both help screens
//!
//! ## Does not own
//! - What the flag accepts and the groups it yields (`mcp_tools_flag_tests`)
//! - The transports themselves (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a transport consumes the selection should normally require
//! changes here. Changes to the accepted values should not.
//!
//! Keep this module focused on the `--tools` wiring.

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
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &clio_mcp::runtime::McpOpenOptions::default(),
            Some(groups),
            std::io::Cursor::new(format!("{request}\n").into_bytes()),
            &mut out,
        ),
        ExitCode::SUCCESS,
        "the session did not serve"
    );
    let response: serde_json::Value =
        serde_json::from_slice(&out).expect("one response line per request");
    let mut names: Vec<String> = response["result"]["tools"]
        .as_array()
        .expect("a served tools/list result carries a tools array")
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
