// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the `--tools` flag's
//! decoding: each preset name, the `custom:` form and its dependency closure,
//! and every value the flag refuses.
//!
//! ## Owns
//! - The group set each accepted preset and `custom:` value yields, written out
//! - The usage refusal for an unknown preset, an unknown group, `all` inside
//!   `custom:`, an empty value, and a bare `custom:`
//! - The absent flag leaving the configured setting to decide
//!
//! ## Does not own
//! - Group membership and the presets themselves (see the `clio-types` tests)
//! - Rendering the refusal, or opening a store (`mcp_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the flag's values.

use super::{HELP_BLOCK, groups, selection};
use clio_types::ToolGroup::{
    self, Graph, Hygiene, ItemRead, ItemWrite, Ops, Persona, Retention, Retrieval, Sync,
};
use clio_types::{ToolGroup as Group, ToolPreset};
use std::process::ExitCode;

/// A usage refusal: exit 2, never an open failure.
fn refused(value: &str) -> crate::cli_error::CliError {
    groups(value).expect_err("the value names no set this build can build")
}

/// The message and the hint of a refusal, joined so one assertion covers both.
fn refusal_text(value: &str) -> String {
    let error = refused(value);
    assert_eq!(
        error.exit_code(),
        ExitCode::from(2),
        "`{value}` must be a usage error"
    );
    let hint = error.hint().unwrap_or_default();
    format!("{} {hint}", error.message())
}

#[test]
fn each_preset_value_yields_that_presets_group_set() {
    assert_eq!(groups("minimal").expect("a preset"), [Retrieval, Graph]);
    assert_eq!(
        groups("coding").expect("a preset"),
        [Retrieval, ItemRead, Persona]
    );
    assert_eq!(
        groups("ops").expect("a preset"),
        [ItemRead, Hygiene, Ops, Retention]
    );
    assert_eq!(groups("all").expect("a preset"), ToolGroup::ALL);
}

#[test]
fn a_preset_value_matches_its_own_resolution() {
    for preset in ToolPreset::ALL {
        assert_eq!(
            groups(preset.as_str()).expect("a preset name"),
            preset.resolve(),
            "`{}` resolves through the shared preset derivation",
            preset.as_str()
        );
    }
}

#[test]
fn a_custom_value_yields_the_minimal_set_plus_the_listed_groups() {
    assert_eq!(
        groups("custom:ops").expect("a group list"),
        [Retrieval, ItemRead, Graph, Ops],
        "the minimal set stays, and `ops` brings the read group it requires"
    );
}

#[test]
fn a_custom_value_adds_what_the_listed_groups_require() {
    // `sync` requires item-read and item-write, and item-write requires
    // item-read again: the closure carries both.
    assert_eq!(
        groups("custom:sync").expect("a group list"),
        [Retrieval, ItemWrite, ItemRead, Graph, Sync]
    );
}

#[test]
fn a_custom_value_ignores_the_space_around_a_group_name() {
    assert_eq!(
        groups("custom: ops , hygiene ").expect("a group list"),
        groups("custom:ops,hygiene").expect("a group list")
    );
}

#[test]
fn an_unknown_preset_is_refused_naming_the_value_and_the_presets() {
    let text = refusal_text("mnimal");
    assert!(text.contains("mnimal"), "{text}");
    for preset in ToolPreset::ALL.map(ToolPreset::as_str) {
        assert!(text.contains(preset), "`{preset}` is not offered: {text}");
    }
}

#[test]
fn an_unknown_group_is_refused_naming_the_value_and_the_group() {
    let text = refusal_text("custom:ops,bogus");
    assert!(text.contains("custom:ops,bogus"), "{text}");
    assert!(text.contains("bogus"), "{text}");
    for group in Group::ALL.map(Group::as_str) {
        assert!(text.contains(group), "`{group}` is not offered: {text}");
    }
}

#[test]
fn all_is_a_preset_and_is_refused_as_a_group() {
    let text = refusal_text("custom:all");
    assert!(text.contains("unknown tool group `all`"), "{text}");
}

#[test]
fn an_empty_value_is_refused_naming_it() {
    let text = refusal_text("");
    assert!(text.contains("--tools ``"), "{text}");
}

#[test]
fn a_bare_custom_prefix_is_refused() {
    let text = refusal_text("custom:");
    assert!(text.contains("custom:"), "{text}");
    assert!(text.contains("names no tool group"), "{text}");
}

#[test]
fn a_trailing_comma_is_refused_rather_than_ignored() {
    let text = refusal_text("custom:ops,");
    assert!(text.contains("names no tool group"), "{text}");
}

#[test]
fn no_flag_leaves_the_configured_setting_to_decide() {
    assert_eq!(
        selection(&crate::cli_flags_test_support::flags_of(&[])).expect("no selection"),
        None
    );
    assert_eq!(
        selection(&crate::cli_flags_test_support::flags_of(&[
            "--tools", "coding"
        ]))
        .expect("a preset"),
        Some(vec![Retrieval, ItemRead, Persona])
    );
}

#[test]
fn the_help_block_states_the_accepted_values_and_the_default() {
    for value in ["--tools", "minimal", "coding", "ops", "all", "custom:"] {
        assert!(HELP_BLOCK.contains(value), "`{value}` is missing");
    }
    assert!(HELP_BLOCK.contains("mcp.tool_profile"), "{}", HELP_BLOCK);
    assert!(HELP_BLOCK.contains("`minimal` preset"), "{}", HELP_BLOCK);
}
