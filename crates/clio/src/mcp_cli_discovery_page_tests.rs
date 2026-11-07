// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what one served default `tools/list` page carries when the
//! discovery gate is on, and what the transport's `--discovery off` switch
//! hides when a configuration layer turns the gate on.
//!
//! ## Owns
//! - A default first page carrying the three discovery tools in declaration
//!   order for the widest preset and for a large custom list
//! - The preset's own first page staying unchanged apart from the three-entry
//!   shift
//! - `--discovery off` reaching the transport's opener and hiding all three
//!   over a configuration layer that turns them on
//!
//! ## Does not own
//! - The flag values and the selections they yield (`mcp_tools_flag_tests`,
//!   `mcp_discovery_flag_tests`)
//! - The listing, the paging, and the gate themselves (see `clio-mcp`)
//! - The served-path helpers and the written-out presets (see `super`)
//!
//! ## Boundary
//!
//! These tests break when a served page's order or the transport's switch
//! wiring changes; they never change either.
//!
//! Keep this module focused on the discovery entries of one served page.

use super::*;

/// One default `tools/list` page carries all three discovery tools, in
/// declaration order, for the widest preset and for a large custom list: the
/// entries are prepended, so the default cap never pushes them out, and the
/// preset's own first page is only shifted by the three.
#[test]
fn a_default_page_carries_the_three_discovery_tools() {
    for value in ["all", "custom:ops"] {
        let groups = selection_of(value);
        let on = ordered_names_in(&served_bytes(Some(&groups), Some(true), TOOLS_LIST));
        let off = ordered_names_in(&served_bytes(Some(&groups), Some(false), TOOLS_LIST));
        assert_eq!(
            on.len(),
            25,
            "`--tools {value}` answered more than one default page: {on:?}"
        );
        let first: Vec<&str> = on[..3].iter().map(String::as_str).collect();
        assert_eq!(
            first,
            ["tool_search", "tool_describe", "tool_invoke"],
            "`--tools {value}` does not carry the three discovery tools first: {on:?}"
        );
        assert_eq!(
            &on[3..],
            &off[..on.len() - 3],
            "`--tools {value}` changed the preset's own page beyond the shift"
        );
        for tool in DISCOVERY_TOOLS {
            assert!(
                !off.contains(&tool.to_owned()),
                "`--tools {value}` advertises `{tool}` with discovery off: {off:?}"
            );
        }
    }
}

/// `--discovery off` reaches the transport's opener and wins over a
/// configuration layer that turns discovery on: no discovery name is served,
/// while the same layer without the switch serves all three.
#[test]
fn discovery_off_wins_over_a_configured_on_through_the_transport() {
    let config = r#"{"mcp": {"discovery": true}}"#;
    let configured_on = names_in(&served_bytes_under(config, None, None, TOOLS_LIST));
    for tool in DISCOVERY_TOOLS {
        assert!(
            configured_on.contains(&tool.to_owned()),
            "the configuration layer turns `{tool}` on: {configured_on:?}"
        );
    }
    let hidden = names_in(&served_bytes_under(config, None, Some(false), TOOLS_LIST));
    for tool in DISCOVERY_TOOLS {
        assert!(
            !hidden.contains(&tool.to_owned()),
            "`--discovery off` must hide `{tool}`: {hidden:?}"
        );
    }
}
