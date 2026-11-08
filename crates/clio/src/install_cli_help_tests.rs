// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the help and success text of the `clio install` and
//! `clio uninstall` surfaces, and the usage errors they share.
//!
//! ## Owns
//! - Assertions over the supported-harness hint, the three usage errors, the
//!   config-surface note, the success text, and both help texts
//!
//! ## Does not own
//! - The text implementation under test (`install_cli_help`)
//! - The harness registry (`agent_harness`)
//!
//! ## Boundary
//!
//! Test-only. Every assertion reads a pure string builder, so no test captures
//! stdout, touches a file, or reads the host environment.

use super::*;
use std::process::ExitCode;

fn record(name: &str) -> &'static HarnessRecord {
    agent_harness::resolve(name).expect("the harness is supported")
}

#[test]
fn supported_hint_lists_every_registered_harness() {
    assert_eq!(
        supported_hint(),
        "supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"
    );
}

#[test]
fn missing_harness_is_a_usage_error_with_the_supported_hint() {
    let error = missing_harness();
    let hint = supported_hint();
    assert_eq!(error.code(), "usage");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert_eq!(error.message(), "--harness is required");
    assert_eq!(error.hint(), Some(hint.as_str()));
}

#[test]
fn unknown_harness_names_the_name_and_lists_support() {
    let error = unknown_harness("vim");
    let hint = supported_hint();
    assert_eq!(error.code(), "usage");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert_eq!(error.message(), "unknown harness `vim`");
    assert_eq!(error.hint(), Some(hint.as_str()));
}

#[test]
fn known_but_unsupported_says_known_rather_than_unknown() {
    let error = known_but_unsupported("cursor");
    let hint = supported_hint();
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.message(),
        "harness `cursor` is known but not yet supported"
    );
    assert!(!error.message().contains("unknown"));
    assert_eq!(error.hint(), Some(hint.as_str()));
}

#[test]
fn the_config_surface_note_names_only_antigravity() {
    let note = config_surface_note(record("antigravity")).expect("antigravity has a note");
    assert_eq!(
        note,
        "note: antigravity writes the CLI global config (~/.gemini/config/mcp_config.json)"
    );
    assert!(config_surface_note(record("claude")).is_none());
    assert!(config_surface_note(record("codex")).is_none());
}

#[test]
fn success_text_appends_the_note_after_the_delegate_message() {
    let text = success_text(record("antigravity"), "registered clio with antigravity");
    assert_eq!(
        text,
        "registered clio with antigravity\nnote: antigravity writes the CLI global config (~/.gemini/config/mcp_config.json)"
    );
    assert_eq!(
        success_text(record("claude"), "registered clio with claude"),
        "registered clio with claude"
    );
}

#[test]
fn install_help_names_the_portable_default_and_an_example() {
    let text = help_text(Verb::Install);
    assert!(text.contains("clio install"), "{text}");
    assert!(
        text.contains("Register the clio MCP server with a supported agent harness."),
        "{text}"
    );
    assert!(
        text.contains(
            "With no --harness, write the portable skill package to ~/.agents/skills/clio-mcp and skip server registration."
        ),
        "{text}"
    );
    assert!(
        text.contains("supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"),
        "{text}"
    );
    assert!(
        text.contains("Example: clio install --harness claude"),
        "{text}"
    );
    assert!(
        text.contains("note: antigravity writes the CLI global config"),
        "{text}"
    );
}

#[test]
fn install_help_states_the_installed_package_identity_once() {
    let sentence = "The skill package directory is clio-mcp, and the same name is the name field in that package's SKILL.md frontmatter.";
    let text = help_text(Verb::Install);
    assert_eq!(
        text.matches(sentence).count(),
        1,
        "the install help names the package identity exactly once: {text}"
    );
    assert!(
        !help_text(Verb::Uninstall).contains(sentence),
        "only the install help carries the package identity"
    );
}

#[test]
fn install_help_states_what_force_replaces_for_a_markerless_package() {
    let text = help_text(Verb::Install);
    assert!(
        text.contains("--force replaces the files the skill package owns"),
        "{text}"
    );
    assert!(
        text.contains(
            "when no marker records a package because an earlier clio installed it, the files whose names that package owns"
        ),
        "{text}"
    );
    assert!(
        text.contains("never deletes a file it does not own and never deletes a skill root"),
        "{text}"
    );
    assert!(
        !text.contains("replaces the skill assets clio recorded, and only those"),
        "the help must not promise force touches only recorded assets: {text}"
    );
}

#[test]
fn uninstall_help_omits_the_portable_default() {
    let text = help_text(Verb::Uninstall);
    assert!(text.contains("clio uninstall"), "{text}");
    assert!(
        text.contains("clio uninstall [--harness <name>] [--skill-only]"),
        "{text}"
    );
    assert!(
        text.contains("Remove the clio MCP server registration from a supported agent harness."),
        "{text}"
    );
    assert!(!text.contains("portable skill package"), "{text}");
    assert!(
        text.contains("With --skill-only, remove only the installed clio-mcp skill package"),
        "{text}"
    );
    assert!(
        text.contains("removes that copy only when no harness still references it"),
        "{text}"
    );
    assert!(
        text.contains("Example: clio uninstall --harness claude"),
        "{text}"
    );
    assert!(
        text.contains("note: antigravity writes the CLI global config"),
        "{text}"
    );
}
